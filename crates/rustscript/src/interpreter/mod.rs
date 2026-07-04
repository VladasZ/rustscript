mod builtins;
mod bytecode;
mod compile;
mod crates_bridge;
mod eval;
mod format;
mod higher_order;
mod http;
mod json_bridge;
mod methods;
mod native;
mod ops;
mod process;
mod regex_bridge;
mod std_bridge;
mod value;
mod vm;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, anyhow, bail};
use syn::{File, Item};

use bytecode::Chunk;
use compile::{Compiler, Ctx};
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
    SCRIPT_ARGS.set(args).expect("script args are set exactly once");
}

pub(crate) fn script_args() -> Vec<String> {
    SCRIPT_ARGS.get().cloned().unwrap_or_default()
}

/// The whole program, compiled to bytecode and ready to run.
pub struct Interp {
    /// Top level functions, indexed by id. Direct calls use the id.
    functions: Vec<Rc<Chunk>>,
    fn_index: HashMap<String, u32>,
    /// Inherent and trait methods, keyed by (type name, method name).
    methods: HashMap<(String, String), Rc<Chunk>>,
    structs: HashMap<String, Rc<syn::ItemStruct>>,
    enums: HashMap<String, Rc<syn::ItemEnum>>,
    uses: HashMap<String, Vec<String>>,
    main_index: Option<u32>,
    /// Lazily built field layouts for user structs, shared across coercions.
    shapes: RefCell<HashMap<String, Rc<eval::Shape>>>,
}

impl Interp {
    pub fn load(file: &File) -> Result<Self> {
        let mut pending_fns: Vec<Rc<syn::ItemFn>> = Vec::new();
        let mut fn_index: HashMap<String, u32> = HashMap::default();
        let mut pending_methods: Vec<(String, String, Rc<syn::ImplItemFn>)> = Vec::new();
        let mut structs: HashMap<String, Rc<syn::ItemStruct>> = HashMap::default();
        let mut enums: HashMap<String, Rc<syn::ItemEnum>> = HashMap::default();
        let mut uses: HashMap<String, Vec<String>> = HashMap::default();

        for item in &file.items {
            match item {
                Item::Fn(f) => {
                    let name = f.sig.ident.to_string();
                    fn_index.insert(name, pending_fns.len() as u32);
                    pending_fns.push(Rc::new(f.clone()));
                }
                Item::Struct(s) => {
                    structs.insert(s.ident.to_string(), Rc::new(s.clone()));
                }
                Item::Enum(e) => {
                    enums.insert(e.ident.to_string(), Rc::new(e.clone()));
                }
                Item::Impl(imp) => {
                    let type_name =
                        type_path_name(&imp.self_ty).ok_or_else(|| anyhow!("unsupported impl target"))?;
                    for it in &imp.items {
                        if let syn::ImplItem::Fn(m) = it {
                            pending_methods.push((
                                type_name.clone(),
                                m.sig.ident.to_string(),
                                Rc::new(m.clone()),
                            ));
                        }
                    }
                }
                Item::Use(u) => {
                    let mut prefix = Vec::new();
                    collect_use_tree(&u.tree, &mut prefix, &mut uses);
                }
                Item::Const(_) | Item::Static(_) | Item::Trait(_) => {}
                Item::Mod(_) => bail!("unsupported feature: nested modules are not run yet"),
                other => bail!("unsupported item: {}", quote_kind(other)),
            }
        }

        let ctx = Ctx { fn_index: fn_index.clone(), structs: structs.clone() };

        let mut functions = Vec::with_capacity(pending_fns.len());
        for f in &pending_fns {
            let mut c = Compiler::new(&ctx);
            functions.push(Rc::new(c.compile_fn(&f.sig, &f.block)?));
        }
        let mut methods = HashMap::default();
        for (ty, name, m) in &pending_methods {
            let mut c = Compiler::new(&ctx);
            methods.insert((ty.clone(), name.clone()), Rc::new(c.compile_fn(&m.sig, &m.block)?));
        }

        let main_index = fn_index.get("main").copied();
        Ok(Interp {
            functions,
            fn_index,
            methods,
            structs,
            enums,
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
        let idx = self.main_index.ok_or_else(|| anyhow!("no `main` function found"))?;
        let chunk = self.functions[idx as usize].clone();
        let ret = self.run_chunk(&chunk, &[], &[])?;
        if let Value::Enum { enum_name, variant, data } = &ret
            && &**enum_name == "Result"
            && &**variant == "Err"
        {
            let msg = data.first().map(|v| v.display()).unwrap_or_default();
            bail!("Error: {msg}");
        }
        Ok(())
    }

    // -- lookups used by the bridge dispatch -------------------------------

    pub(super) fn user_function(&self, name: &str) -> Option<Rc<Chunk>> {
        self.fn_index.get(name).map(|&i| self.functions[i as usize].clone())
    }

    pub(super) fn user_method(&self, ty: &str, name: &str) -> Option<Rc<Chunk>> {
        self.methods.get(&(ty.to_string(), name.to_string())).cloned()
    }

    pub(super) fn structs(&self) -> &HashMap<String, Rc<syn::ItemStruct>> {
        &self.structs
    }
}

fn collect_use_tree(
    tree: &syn::UseTree,
    prefix: &mut Vec<String>,
    out: &mut HashMap<String, Vec<String>>,
) {
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            collect_use_tree(&p.tree, prefix, out);
            prefix.pop();
        }
        syn::UseTree::Name(n) => {
            let name = n.ident.to_string();
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
                collect_use_tree(item, prefix, out);
            }
        }
        syn::UseTree::Glob(_) => {}
    }
}

fn type_path_name(ty: &syn::Type) -> Option<String> {
    if let syn::Type::Path(p) = ty {
        p.path.segments.last().map(|s| s.ident.to_string())
    } else {
        None
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
