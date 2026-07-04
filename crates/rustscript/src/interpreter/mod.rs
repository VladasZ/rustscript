mod builtins;
mod eval;
mod format;
mod native;
mod value;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, anyhow, bail};
use syn::{File, Item};

pub use value::Value;

/// Set by the real Ctrl-C handler, which must stay `Send`, and drained by the
/// interpreter between statements so it can run the script's own handler.
static CTRLC_HIT: AtomicBool = AtomicBool::new(false);
static CTRLC_INSTALLED: OnceLock<bool> = OnceLock::new();

thread_local! {
    /// The script closure passed to `ctrlc::set_handler`, run on the interpreter
    /// thread when a Ctrl-C is noticed.
    static CTRLC_HANDLER: RefCell<Option<Value>> = const { RefCell::new(None) };
}

/// Register the script's Ctrl-C closure and install the OS handler once.
pub(crate) fn set_ctrlc_handler(closure: Value) -> Result<()> {
    CTRLC_HANDLER.with(|h| *h.borrow_mut() = Some(closure));
    if CTRLC_INSTALLED.set(true).is_ok() {
        ctrlc::set_handler(|| CTRLC_HIT.store(true, Ordering::SeqCst))
            .map_err(|e| anyhow!("could not install ctrl-c handler: {e}"))?;
    }
    Ok(())
}

/// The arguments a script sees through `std::env::args()`. Set once from
/// `main` before the script runs. Index 0 is the script path, matching a real
/// compiled binary, then the arguments typed after the filename.
static SCRIPT_ARGS: OnceLock<Vec<String>> = OnceLock::new();

pub fn set_script_args(args: Vec<String>) {
    SCRIPT_ARGS
        .set(args)
        .expect("script args are set exactly once");
}

pub(crate) fn script_args() -> Vec<String> {
    SCRIPT_ARGS.get().cloned().unwrap_or_default()
}

/// Control flow result of evaluating a statement or expression.
pub enum Flow {
    Value(Value),
    Return(Value),
    Break(Value),
    Continue,
}

/// Unwrap a `Flow` to its value, bubbling any control signal up to the caller.
macro_rules! flow {
    ($e:expr) => {
        match $e? {
            $crate::interpreter::Flow::Value(v) => v,
            other => return Ok(other),
        }
    };
}
pub(crate) use flow;

/// A single call frame. A stack of lexical scopes, innermost last.
pub struct Frame {
    scopes: Vec<HashMap<String, Value>>,
}

impl Frame {
    fn new() -> Self {
        Frame {
            scopes: vec![HashMap::new()],
        }
    }

    fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop(&mut self) {
        self.scopes.pop();
    }

    fn define(&mut self, name: &str, val: Value) {
        self.scopes.last_mut().unwrap().insert(name.to_string(), val);
    }

    fn get(&self, name: &str) -> Option<Value> {
        self.scopes
            .iter()
            .rev()
            .find_map(|s| s.get(name))
            .cloned()
    }

    /// Flatten all visible bindings, inner scopes shadowing outer, for a
    /// closure to capture.
    fn snapshot(&self) -> HashMap<String, Value> {
        let mut out = HashMap::new();
        for scope in &self.scopes {
            for (k, v) in scope {
                out.insert(k.clone(), v.clone());
            }
        }
        out
    }

    fn set(&mut self, name: &str, val: Value) -> bool {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(slot) = scope.get_mut(name) {
                *slot = val;
                return true;
            }
        }
        false
    }
}

/// The whole program, with its items indexed for lookup during evaluation.
pub struct Interp {
    functions: HashMap<String, Rc<syn::ItemFn>>,
    structs: HashMap<String, Rc<syn::ItemStruct>>,
    enums: HashMap<String, Rc<syn::ItemEnum>>,
    /// Inherent and trait methods, keyed by (type name, method name).
    methods: HashMap<(String, String), Rc<syn::ImplItemFn>>,
    /// Imported names mapped to their full path, so `fs::read` can be resolved
    /// back to `scriptstd::fs::read` for native bridge dispatch.
    uses: HashMap<String, Vec<String>>,
}

impl Interp {
    pub fn load(file: &File) -> Result<Self> {
        let mut interp = Interp {
            functions: HashMap::new(),
            structs: HashMap::new(),
            enums: HashMap::new(),
            methods: HashMap::new(),
            uses: HashMap::new(),
        };
        for item in &file.items {
            interp.collect_item(item)?;
        }
        Ok(interp)
    }

    fn collect_item(&mut self, item: &Item) -> Result<()> {
        match item {
            Item::Fn(f) => {
                self.functions
                    .insert(f.sig.ident.to_string(), Rc::new(f.clone()));
            }
            Item::Struct(s) => {
                self.structs
                    .insert(s.ident.to_string(), Rc::new(s.clone()));
            }
            Item::Enum(e) => {
                self.enums.insert(e.ident.to_string(), Rc::new(e.clone()));
            }
            Item::Impl(imp) => {
                let type_name = type_path_name(&imp.self_ty)
                    .ok_or_else(|| anyhow!("unsupported impl target"))?;
                for it in &imp.items {
                    if let syn::ImplItem::Fn(m) = it {
                        self.methods.insert(
                            (type_name.clone(), m.sig.ident.to_string()),
                            Rc::new(m.clone()),
                        );
                    }
                }
            }
            Item::Use(u) => {
                let mut prefix = Vec::new();
                collect_use_tree(&u.tree, &mut prefix, &mut self.uses);
            }
            Item::Const(_) | Item::Static(_) => {}
            Item::Mod(_) => bail!("unsupported feature: nested modules are not run yet"),
            Item::Trait(_) => {}
            other => bail!(
                "unsupported item: {}",
                quote_kind(other)
            ),
        }
        Ok(())
    }

    /// If a Ctrl-C arrived, run the script's registered handler closure. Called
    /// between statements so the handler runs on the interpreter thread.
    pub(super) fn run_pending_ctrlc(&self) -> Result<()> {
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
        let main = self
            .functions
            .get("main")
            .ok_or_else(|| anyhow!("no `main` function found"))?
            .clone();
        let mut frame = Frame::new();
        let ret = self.call_fn_body(&main.block, &main.sig, &[], &mut frame)?;
        if let Value::Enum {
            enum_name,
            variant,
            data,
        } = &ret
            && enum_name == "Result"
            && variant == "Err"
        {
            let msg = data.borrow().first().map(|v| v.display()).unwrap_or_default();
            bail!("Error: {msg}");
        }
        Ok(())
    }
}

/// Flatten a `use` tree into `name -> full path` entries. `use a::b::c;`
/// records `c -> [a, b, c]`. Groups and globs are walked. Renames use the alias.
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
