mod assoc;
mod bridge;
mod bytecode;
mod compile;
mod console;
pub mod coverage;
mod crates_bridge;
mod format;
mod higher_order;
mod http;
mod int_methods;
mod iterator;
mod json_bridge;
mod jwt_bridge;
mod methods;
mod native;
mod native_methods;
mod numeric;
mod ops;
mod pdf_bridge;
mod process;
mod ratatui;
mod ratatui_bridge;
mod ratatui_render;
mod regex_bridge;
mod resolver;
mod serde_attrs;
mod service_bridge;
mod shared;
mod std_bridge;
mod typeir;
mod value;
mod vecmap;
mod vm;
mod vm_support;
mod winreg_bridge;
mod wmi_bridge;
mod xmltree_bridge;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, anyhow, bail};
use syn::Item;

use crate::loader::ModuleSrc;
use bytecode::Chunk;
use compile::{Compiler, Ctx};
use resolver::{ModuleSyms, Res, Resolver, StructDef};
pub use vm_support::{ErrReturn, ScriptPanic};

/// Set by the real Ctrl-C handler, which must stay `Send`, and drained by the
/// interpreter between loop iterations so it can run the script's own handler.
static CTRLC_HIT: AtomicBool = AtomicBool::new(false);
static CTRLC_INSTALLED: OnceLock<bool> = OnceLock::new();
static CTRLC_HANDLER: parking_lot::Mutex<Option<value::Value>> = parking_lot::Mutex::new(None);

pub(crate) fn set_ctrlc_handler(closure: value::Value) -> Result<()> {
    *CTRLC_HANDLER.lock() = Some(closure);
    if CTRLC_INSTALLED.set(true).is_ok() {
        ctrlc::set_handler(|| CTRLC_HIT.store(true, Ordering::SeqCst))
            .map_err(|e| anyhow!("could not install ctrl-c handler: {e}"))?;
    }
    Ok(())
}

/// The script's Ctrl-C handler when a Ctrl-C is pending, None otherwise.
/// Draining the flag here means each Ctrl-C runs the handler once.
pub(crate) fn pending_ctrlc_handler() -> Option<value::Value> {
    // Cheap relaxed load first, this runs on every loop iteration.
    if !CTRLC_HIT.load(Ordering::Relaxed) {
        return None;
    }
    if !CTRLC_HIT.swap(false, Ordering::SeqCst) {
        return None;
    }
    CTRLC_HANDLER.lock().clone()
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

/// Run a program. Every script runs on the one engine, a multi thread tokio
/// runtime whose values are backed by `Arc`. `async_mode` marks the script as
/// `#[tokio::main]`, which is what allows `.await`, `tokio::spawn`, and
/// `join!` to compile.
pub fn run(modules: &[ModuleSrc], async_mode: bool) -> Result<()> {
    let interp = Interp::load(modules, async_mode)?;
    interp.run()
}

/// A module level const or static: compiled once, evaluated on first read.
enum GlobalSlot {
    Todo(Arc<Chunk>),
}

/// The whole program, compiled to bytecode and ready to run.
pub struct Interp {
    /// Every function of every module, indexed by id. Direct calls use the id.
    functions: Vec<Arc<Chunk>>,
    /// Canonical name to function id, for calls resolved at runtime.
    fn_index: HashMap<String, u32>,
    /// Inherent and trait methods, keyed by (canonical type name, method name).
    methods: HashMap<(String, String), Arc<Chunk>>,
    /// Module tree and item tables, shared by compile and runtime lookups.
    resolver: Resolver,
    /// Consts and statics, evaluated lazily so declaration order is free.
    globals: RefCell<Vec<GlobalSlot>>,
    /// Root module imports, used by the bridge dispatch to expand aliases.
    uses: HashMap<String, Vec<String>>,
    main_index: Option<u32>,
}

/// Stated return scalars of the script's own functions, one more place a
/// `Default` payload is written down. A name defined in more than one module
/// with differing returns answers for neither.
fn collect_fn_returns(
    pending_fns: &[(usize, Rc<syn::ItemFn>)],
) -> HashMap<String, bytecode::ScalarTy> {
    let mut seen_returns: HashMap<String, Option<bytecode::ScalarTy>> = HashMap::default();
    for (_, f) in pending_fns {
        let lowered = match &f.sig.output {
            syn::ReturnType::Type(_, ty) => bytecode::ScalarTy::lower(ty),
            syn::ReturnType::Default => None,
        };
        seen_returns
            .entry(f.sig.ident.to_string())
            .and_modify(|known| {
                if *known != lowered {
                    *known = None;
                }
            })
            .or_insert(lowered);
    }
    seen_returns
        .into_iter()
        .filter_map(|(name, scalar)| scalar.map(|s| (name, s)))
        .collect()
}

/// Every function under its full `module::name` key, bare names at the root.
fn build_fn_index(resolver: &Resolver) -> HashMap<String, u32> {
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
    fn_index
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

        let fn_returns = collect_fn_returns(&pending_fns);

        let mut functions = Vec::with_capacity(pending_fns.len());
        for (m, f) in &pending_fns {
            let ctx = Ctx {
                resolver: &resolver,
                module: *m,
                file: modules[*m].file.clone(),
                async_mode,
                impl_type: None,
                fn_returns: &fn_returns,
            };
            let mut c = Compiler::new(&ctx);
            functions.push(Arc::new(c.compile_fn(&f.sig, &f.block)?));
        }
        let mut methods = HashMap::default();
        for (ty, name, m, f) in &pending_methods {
            let ctx = Ctx {
                resolver: &resolver,
                module: *m,
                file: modules[*m].file.clone(),
                async_mode,
                impl_type: Some(ty),
                fn_returns: &fn_returns,
            };
            let mut c = Compiler::new(&ctx);
            methods.insert(
                (ty.clone(), name.clone()),
                Arc::new(c.compile_fn(&f.sig, &f.block)?),
            );
        }
        let mut globals = Vec::with_capacity(pending_consts.len());
        for (m, expr) in &pending_consts {
            let ctx = Ctx {
                resolver: &resolver,
                module: *m,
                file: modules[*m].file.clone(),
                async_mode,
                impl_type: None,
                fn_returns: &fn_returns,
            };
            let mut c = Compiler::new(&ctx);
            globals.push(GlobalSlot::Todo(Arc::new(c.compile_const(expr)?)));
        }

        let fn_index = build_fn_index(&resolver);
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
        })
    }

    /// Run `fn main`. Its returned `Result::Err` is reported like anyhow does.
    /// Report methods the interpreter does not implement, without running
    /// anything. Used by `rust check`.
    pub fn coverage(&self) -> Vec<coverage::Finding> {
        let user = self.methods.keys().map(|(_, m)| m.clone());
        coverage::report(&self.functions, user)
    }

    /// Run `main` as a blocking task on a multi thread tokio runtime, so
    /// awaited futures can be driven with `block_on` from a worker thread.
    fn run(&self) -> Result<()> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| anyhow!("cannot start tokio runtime: {e}"))?;
        let functions = self.functions.clone();
        let methods = self.methods.clone();
        let globals: Vec<parking_lot::Mutex<vm::GlobalSlot>> = self
            .globals
            .borrow()
            .iter()
            .map(|slot| {
                let GlobalSlot::Todo(c) = slot;
                parking_lot::Mutex::new(vm::GlobalSlot::Todo(c.clone()))
            })
            .collect();
        // The runtime tables for dynamic dispatch, precomputed here so nothing
        // at runtime touches the syn AST, which is not `Send`.
        let enums: Vec<vm::EnumDef> = self
            .resolver
            .enums
            .iter()
            .map(|(name, def)| vm::EnumDef {
                name: Arc::from(&**name),
                variants: def
                    .variants
                    .iter()
                    .map(|v| {
                        (
                            Arc::from(v.ident.to_string().as_str()),
                            matches!(v.fields, syn::Fields::Unit),
                        )
                    })
                    .collect(),
            })
            .collect();
        let unit_structs: Vec<Arc<str>> = self
            .resolver
            .structs
            .iter()
            .filter(|(_, def)| matches!(def.ast.fields, syn::Fields::Unit))
            .map(|(name, _)| Arc::from(&**name))
            .collect();
        let struct_names: std::collections::HashSet<String> = self
            .resolver
            .structs
            .keys()
            .map(ToString::to_string)
            .collect();
        let pinterp = Arc::new(vm::Vm {
            functions,
            fn_index: self.fn_index.clone(),
            methods,
            globals,
            structs: self.build_structs(),
            uses: self.uses.clone(),
            enums,
            unit_structs,
            struct_names,
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
        if let value::Value::Enum {
            enum_name,
            variant,
            data,
        } = &ret
            && &**enum_name == "Result"
            && &**variant == "Err"
        {
            let msg = data.first().map(value::Value::display).unwrap_or_default();
            return Err(anyhow::Error::new(vm_support::ErrReturn(msg)));
        }
        Ok(())
    }

    fn structs(&self) -> &HashMap<Arc<str>, StructDef> {
        &self.resolver.structs
    }

    fn resolver(&self) -> &Resolver {
        &self.resolver
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
            resolver.modules[m].fns.insert(
                name,
                u32::try_from(pending_fns.len()).expect("table fits u32"),
            );
            pending_fns.push((m, Rc::new(f.clone())));
        }
        Item::Struct(s) => {
            let name = s.ident.to_string();
            let canon: Arc<str> = resolver.canon(m, &name).into();
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
            let canon: Arc<str> = resolver.canon(m, &name).into();
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
            resolver.modules[m].consts.insert(
                c.ident.to_string(),
                u32::try_from(pending_consts.len()).expect("table fits u32"),
            );
            pending_consts.push((m, Rc::new((*c.expr).clone())));
        }
        Item::Static(s) => {
            if matches!(s.mutability, syn::StaticMutability::Mut(_)) {
                bail!("unsupported feature: `static mut`");
            }
            resolver.modules[m].consts.insert(
                s.ident.to_string(),
                u32::try_from(pending_consts.len()).expect("table fits u32"),
            );
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
