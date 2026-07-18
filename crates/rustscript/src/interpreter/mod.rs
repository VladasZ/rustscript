mod builtins;
mod bytecode;
mod compile;
mod crates_bridge;
mod docx_bridge;
mod eval;
mod format;
mod higher_order;
mod http;
mod iterator;
mod json_bridge;
mod jwt_bridge;
mod methods;
mod native;
mod ops;
mod pbridge;
mod pchunk;
mod pdf_bridge;
mod phttp;
mod pnative;
mod pops;
mod process;
mod pvalue;
mod pvm;
mod regex_bridge;
mod resolver;
mod runner;
mod std_bridge;
mod value;
mod vm;
mod vm_support;

use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::replace;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, anyhow, bail};
use syn::Item;

use crate::loader::ModuleSrc;
use bytecode::Chunk;
use compile::{Compiler, Ctx};
use resolver::{ModuleSyms, Res, Resolver, StructDef};
pub use value::Value;

/// Set by the real Ctrl-C handler, which must stay `Send`, and drained by the
/// interpreter between loop iterations so it can run the script's own handler.
static CTRLC_HIT: AtomicBool = AtomicBool::new(false);
static CTRLC_INSTALLED: OnceLock<bool> = OnceLock::new();

thread_local! {
    static CTRLC_HANDLER: RefCell<Option<Value>> = const { RefCell::new(None) };
}

pub(crate) fn set_ctrlc_handler(closure: Value) -> Result<()> {
    CTRLC_HANDLER.with(|h| *h.borrow_mut() = Some(closure));
    if CTRLC_INSTALLED.set(true).is_ok() {
        ctrlc::set_handler(|| CTRLC_HIT.store(true, Ordering::SeqCst))
            .map_err(|e| anyhow!("could not install ctrl-c handler: {e}"))?;
    }
    Ok(())
}

/// The arguments a script sees through `std::env::args()`. Index 0 is the
/// script path, matching a real compiled binary.
static SCRIPT_ARGS: OnceLock<Vec<String>> = OnceLock::new();

pub fn set_script_args(args: Vec<String>) {
    SCRIPT_ARGS
        .set(args)
        .expect("script args are set exactly once");
}

pub(crate) fn script_args() -> Vec<String> {
    SCRIPT_ARGS.get().cloned().unwrap_or_default()
}

/// Entry point for `#[tokio::main]` scripts. These run on the parallel engine
/// with a real multi thread tokio runtime, values backed by `Arc` so tasks move
/// across threads.
pub fn run_parallel(modules: &[ModuleSrc]) -> Result<()> {
    let interp = Interp::load(modules, true)?;
    interp.run_parallel()
}

/// A module level const or static: compiled once, evaluated on first read.
enum GlobalSlot {
    Todo(Rc<Chunk>),
    Busy,
    Ready(Value),
}

/// The whole program, compiled to bytecode and ready to run.
pub struct Interp {
    /// Every function of every module, indexed by id. Direct calls use the id.
    functions: Vec<Rc<Chunk>>,
    /// Canonical name to function id, for calls resolved at runtime.
    fn_index: HashMap<String, u32>,
    /// Inherent and trait methods, keyed by (canonical type name, method name).
    methods: HashMap<(String, String), Rc<Chunk>>,
    /// Module tree and item tables, shared by compile and runtime lookups.
    resolver: Resolver,
    /// Consts and statics, evaluated lazily so declaration order is free.
    globals: RefCell<Vec<GlobalSlot>>,
    /// Root module imports, used by the bridge dispatch to expand aliases.
    uses: HashMap<String, Vec<String>>,
    main_index: Option<u32>,
    /// Lazily built field layouts for user structs, shared across coercions.
    shapes: RefCell<HashMap<Rc<str>, Rc<eval::Shape>>>,
}

impl Interp {
    pub fn load(modules: &[ModuleSrc], async_mode: bool) -> Result<Self> {
        let mut resolver = build_module_tree(modules);
        let mut pending_fns: Vec<(usize, Rc<syn::ItemFn>)> = Vec::new();
        let mut pending_impls: Vec<(usize, Rc<syn::ItemImpl>)> = Vec::new();
        let mut pending_consts: Vec<(usize, Rc<syn::Expr>)> = Vec::new();

        for (m, src) in modules.iter().enumerate() {
            for item in &src.items {
                register_item(
                    &mut resolver,
                    m,
                    item,
                    &mut pending_fns,
                    &mut pending_impls,
                    &mut pending_consts,
                )?;
            }
        }
        resolver.reject_module_globs()?;

        // Impl targets resolve only after every module registered its types.
        let mut pending_methods: Vec<(String, String, usize, Rc<syn::ImplItemFn>)> = Vec::new();
        for (m, imp) in &pending_impls {
            let type_name = impl_target(&resolver, *m, &imp.self_ty)
                .ok_or_else(|| anyhow!("unsupported impl target"))?;
            for it in &imp.items {
                if let syn::ImplItem::Fn(f) = it {
                    pending_methods.push((
                        type_name.clone(),
                        f.sig.ident.to_string(),
                        *m,
                        Rc::new(f.clone()),
                    ));
                }
            }
        }

        let mut functions = Vec::with_capacity(pending_fns.len());
        for (m, f) in &pending_fns {
            let ctx = Ctx {
                resolver: &resolver,
                module: *m,
                async_mode,
            };
            let mut c = Compiler::new(&ctx);
            functions.push(Rc::new(c.compile_fn(&f.sig, &f.block)?));
        }
        let mut methods = HashMap::default();
        for (ty, name, m, f) in &pending_methods {
            let ctx = Ctx {
                resolver: &resolver,
                module: *m,
                async_mode,
            };
            let mut c = Compiler::new(&ctx);
            methods.insert(
                (ty.clone(), name.clone()),
                Rc::new(c.compile_fn(&f.sig, &f.block)?),
            );
        }
        let mut globals = Vec::with_capacity(pending_consts.len());
        for (m, expr) in &pending_consts {
            let ctx = Ctx {
                resolver: &resolver,
                module: *m,
                async_mode,
            };
            let mut c = Compiler::new(&ctx);
            globals.push(GlobalSlot::Todo(Rc::new(c.compile_const(expr)?)));
        }

        let mut fn_index = HashMap::default();
        for syms in &resolver.modules {
            let prefix = syms.path.join("::");
            for (name, &idx) in &syms.fns {
                let key = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}::{name}")
                };
                fn_index.insert(key, idx);
            }
        }
        let main_index = resolver.modules[0].fns.get("main").copied();
        let uses = resolver.modules[0].uses.clone();
        Ok(Interp {
            functions,
            fn_index,
            methods,
            resolver,
            globals: RefCell::new(globals),
            uses,
            main_index,
            shapes: RefCell::new(HashMap::default()),
        })
    }

    /// If a Ctrl-C arrived, run the script's registered handler closure.
    pub(super) fn run_pending_ctrlc(&self) -> Result<()> {
        // Cheap relaxed load first, this runs on every loop iteration.
        if !CTRLC_HIT.load(Ordering::Relaxed) {
            return Ok(());
        }
        if !CTRLC_HIT.swap(false, Ordering::SeqCst) {
            return Ok(());
        }
        let handler = CTRLC_HANDLER.with(|h| h.borrow().clone());
        if let Some(Value::Closure(clo)) = handler {
            self.call_closure(&clo, &[])?;
        }
        Ok(())
    }

    /// Run `fn main`. Its returned `Result::Err` is reported like anyhow does.
    pub fn run_main(&self) -> Result<()> {
        let idx = self
            .main_index
            .ok_or_else(|| anyhow!("no `main` function found"))?;
        let chunk = self.functions[idx as usize].clone();
        let ret = self.run_chunk(&chunk, &[], &[])?;
        if let Value::Enum {
            enum_name,
            variant,
            data,
        } = &ret
            && &**enum_name == "Result"
            && &**variant == "Err"
        {
            let msg = data.first().map(|v| v.display()).unwrap_or_default();
            bail!("Error: {msg}");
        }
        Ok(())
    }

    /// Run a `#[tokio::main]` program on the parallel engine. Compiles once to
    /// the fast bytecode, converts it to `Arc` based `PChunk`, then runs `main`
    /// as a blocking task on a multi thread tokio runtime so awaited futures can
    /// be driven with `block_on` from a worker.
    fn run_parallel(&self) -> Result<()> {
        use std::sync::Arc;
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| anyhow!("cannot start tokio runtime: {e}"))?;
        let functions: Vec<Arc<pchunk::PChunk>> =
            self.functions.iter().map(|c| pchunk::convert(c)).collect();
        let methods = self
            .methods
            .iter()
            .map(|(k, c)| (k.clone(), pchunk::convert(c)))
            .collect();
        let globals: Vec<parking_lot::Mutex<pvm::PGlobalSlot>> = self
            .globals
            .borrow()
            .iter()
            .map(|slot| {
                let g = match slot {
                    GlobalSlot::Todo(c) => pvm::PGlobalSlot::Todo(pchunk::convert(c)),
                    _ => pvm::PGlobalSlot::Busy,
                };
                parking_lot::Mutex::new(g)
            })
            .collect();
        let pinterp = Arc::new(pvm::PInterp {
            functions,
            fn_index: self.fn_index.clone(),
            methods,
            globals,
            rt: rt.handle().clone(),
        });
        let idx = self
            .main_index
            .ok_or_else(|| anyhow!("no `main` function found"))? as usize;
        let main_chunk = pinterp.functions[idx].clone();
        let runner = pinterp.clone();
        let joined = rt.block_on(async move {
            tokio::task::spawn_blocking(move || runner.run_chunk(&main_chunk, &[], &[])).await
        });
        let ret = joined.map_err(|e| anyhow!("main task panicked: {e}"))??;
        if let pvalue::PValue::Enum {
            enum_name,
            variant,
            data,
        } = &ret
            && &**enum_name == "Result"
            && &**variant == "Err"
        {
            let msg = data.first().map(|v| v.display()).unwrap_or_default();
            bail!("Error: {msg}");
        }
        Ok(())
    }

    // -- lookups used by the bridge dispatch and the VM ---------------------

    pub(super) fn user_function(&self, name: &str) -> Option<Rc<Chunk>> {
        self.fn_index
            .get(name)
            .map(|&i| self.functions[i as usize].clone())
    }

    pub(super) fn user_method(&self, ty: &str, name: &str) -> Option<Rc<Chunk>> {
        self.methods
            .get(&(ty.to_string(), name.to_string()))
            .cloned()
    }

    fn structs(&self) -> &HashMap<Rc<str>, StructDef> {
        &self.resolver.structs
    }

    fn enums(&self) -> &HashMap<Rc<str>, Rc<syn::ItemEnum>> {
        &self.resolver.enums
    }

    fn resolver(&self) -> &Resolver {
        &self.resolver
    }

    /// Value of a const or static, evaluated on first use so cross module
    /// declaration order never matters.
    fn global(&self, idx: usize) -> Result<Value> {
        {
            let globals = self.globals.borrow();
            match &globals[idx] {
                GlobalSlot::Ready(v) => return Ok(v.clone()),
                GlobalSlot::Busy => bail!("constant initializers depend on each other in a cycle"),
                GlobalSlot::Todo(_) => {}
            }
        }
        let chunk = {
            let mut globals = self.globals.borrow_mut();
            match replace(&mut globals[idx], GlobalSlot::Busy) {
                GlobalSlot::Todo(c) => c,
                other => {
                    globals[idx] = other;
                    bail!("constant initializers depend on each other in a cycle");
                }
            }
        };
        let v = self.run_chunk(&chunk, &[], &[])?;
        self.globals.borrow_mut()[idx] = GlobalSlot::Ready(v.clone());
        Ok(v)
    }
}

/// Build the empty module table with parent and child links.
fn build_module_tree(modules: &[ModuleSrc]) -> Resolver {
    let index: HashMap<String, usize> = modules
        .iter()
        .enumerate()
        .map(|(i, m)| (m.path.join("::"), i))
        .collect();
    let mut syms: Vec<ModuleSyms> = modules
        .iter()
        .map(|m| ModuleSyms {
            path: m.path.clone(),
            ..ModuleSyms::default()
        })
        .collect();
    for (i, m) in modules.iter().enumerate() {
        if let Some((name, parent_path)) = m.path.split_last() {
            let parent = index[&parent_path.join("::")];
            syms[i].parent = Some(parent);
            syms[parent].children.insert(name.clone(), i);
        }
    }
    Resolver {
        modules: syms,
        structs: HashMap::default(),
        enums: HashMap::default(),
    }
}

fn register_item(
    resolver: &mut Resolver,
    m: usize,
    item: &Item,
    pending_fns: &mut Vec<(usize, Rc<syn::ItemFn>)>,
    pending_impls: &mut Vec<(usize, Rc<syn::ItemImpl>)>,
    pending_consts: &mut Vec<(usize, Rc<syn::Expr>)>,
) -> Result<()> {
    match item {
        Item::Fn(f) => {
            let name = f.sig.ident.to_string();
            resolver.modules[m]
                .fns
                .insert(name, pending_fns.len() as u32);
            pending_fns.push((m, Rc::new(f.clone())));
        }
        Item::Struct(s) => {
            let name = s.ident.to_string();
            let canon: Rc<str> = resolver.canon(m, &name).into();
            resolver.modules[m].structs.insert(name, canon.clone());
            resolver.structs.insert(
                canon,
                StructDef {
                    ast: Rc::new(s.clone()),
                    module: m,
                },
            );
        }
        Item::Enum(e) => {
            let name = e.ident.to_string();
            let canon: Rc<str> = resolver.canon(m, &name).into();
            resolver.modules[m].enums.insert(name, canon.clone());
            resolver.enums.insert(canon, Rc::new(e.clone()));
        }
        Item::Impl(imp) => pending_impls.push((m, Rc::new(imp.clone()))),
        Item::Use(u) => {
            let syms = &mut resolver.modules[m];
            let mut prefix = Vec::new();
            collect_use_tree(&u.tree, &mut prefix, &mut syms.uses, &mut syms.globs);
        }
        Item::Const(c) => {
            resolver.modules[m]
                .consts
                .insert(c.ident.to_string(), pending_consts.len() as u32);
            pending_consts.push((m, Rc::new((*c.expr).clone())));
        }
        Item::Static(s) => {
            if matches!(s.mutability, syn::StaticMutability::Mut(_)) {
                bail!("unsupported feature: `static mut`");
            }
            resolver.modules[m]
                .consts
                .insert(s.ident.to_string(), pending_consts.len() as u32);
            pending_consts.push((m, Rc::new((*s.expr).clone())));
        }
        Item::Type(t) => {
            resolver.modules[m]
                .aliases
                .insert(t.ident.to_string(), Rc::new((*t.ty).clone()));
        }
        Item::Trait(_) => {}
        Item::Mod(_) => bail!("module declarations must be expanded by the loader"),
        other => bail!("unsupported item: {}", quote_kind(other)),
    }
    Ok(())
}

/// Canonical name of the type an `impl` block targets.
fn impl_target(resolver: &Resolver, m: usize, ty: &syn::Type) -> Option<String> {
    let syn::Type::Path(p) = ty else { return None };
    let segs: Vec<String> = p
        .path
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect();
    match resolver.resolve(m, &segs) {
        Ok(Res::Struct(c) | Res::Enum(c)) => Some(c.to_string()),
        // An impl on something else, a bridge type name for example, keeps
        // the old bare name behavior.
        _ => segs.last().cloned(),
    }
}

fn collect_use_tree(
    tree: &syn::UseTree,
    prefix: &mut Vec<String>,
    out: &mut HashMap<String, Vec<String>>,
    globs: &mut Vec<Vec<String>>,
) {
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            collect_use_tree(&p.tree, prefix, out, globs);
            prefix.pop();
        }
        syn::UseTree::Name(n) => {
            let name = n.ident.to_string();
            if name == "self" {
                // `use a::b::{self}` imports the module under its own name.
                if let Some(last) = prefix.last() {
                    out.insert(last.clone(), prefix.clone());
                }
                return;
            }
            let mut full = prefix.clone();
            full.push(name.clone());
            out.insert(name, full);
        }
        syn::UseTree::Rename(r) => {
            let mut full = prefix.clone();
            full.push(r.ident.to_string());
            out.insert(r.rename.to_string(), full);
        }
        syn::UseTree::Group(g) => {
            for item in &g.items {
                collect_use_tree(item, prefix, out, globs);
            }
        }
        syn::UseTree::Glob(_) => globs.push(prefix.clone()),
    }
}

fn quote_kind(item: &Item) -> &'static str {
    match item {
        Item::Fn(_) => "fn",
        Item::Struct(_) => "struct",
        Item::Enum(_) => "enum",
        Item::Impl(_) => "impl",
        Item::Trait(_) => "trait",
        Item::Macro(_) => "macro",
        Item::Union(_) => "union",
        _ => "item",
    }
}
