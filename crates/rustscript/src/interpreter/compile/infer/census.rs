//! The census of calls the inference pass could not type. A method that dispatches but has no row
//! in the tables of this pass gives `Unknown`, and the runtime then goes by what the value says.
//! That is where a lost integer width or a wrong `Default` comes from, so `rust types` lists
//! every such call. A call on a receiver that is itself unknown is not listed, its cause sits
//! earlier in the chain.

use std::mem::take;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use syn::spanned::Spanned;

use super::{Infer, Ty};

const NEVER: [&str; 2] = ["process", "exit"];

static ENABLED: AtomicBool = AtomicBool::new(false);
static FOUND: Mutex<Vec<Untyped>> = Mutex::new(Vec::new());

/// One call whose result the pass left `Unknown`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Untyped {
    pub file: String,
    pub line: usize,
    /// the receiver kind, or `path` for a call by path
    pub receiver: String,
    pub name: String,
}

/// A call the pass saw, checked against the final table in `finish`.
pub(super) struct Site {
    node: *const (),
    line: usize,
    receiver: Option<Ty>,
    name: String,
}

pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// Every untyped call found since `enable`, sorted, each once.
pub fn found() -> Vec<Untyped> {
    let mut found = take(&mut *FOUND.lock());
    found.sort();
    found.dedup();
    found
}

impl Infer<'_, '_> {
    pub(super) fn note_method(&mut self, m: &syn::ExprMethodCall, receiver: &Ty) {
        if !ENABLED.load(Ordering::Relaxed) {
            return;
        }
        self.call_sites.push(Site {
            node: std::ptr::from_ref(m).cast(),
            line: m.method.span().start().line,
            receiver: Some(receiver.clone()),
            name: m.method.to_string(),
        });
    }

    pub(super) fn note_path_call(&mut self, c: &syn::ExprCall, segs: &[String]) {
        // `exit` returns `!`, there is no type to know
        if !ENABLED.load(Ordering::Relaxed) || segs.ends_with(&NEVER.map(String::from)) {
            return;
        }
        self.call_sites.push(Site {
            node: std::ptr::from_ref(c).cast(),
            line: c.func.span().start().line,
            receiver: None,
            name: segs.join("::"),
        });
    }

    /// Reports the noted calls the finished table still holds as `Unknown`.
    pub(super) fn report_untyped(&self) {
        if self.call_sites.is_empty() {
            return;
        }
        let mut found = FOUND.lock();
        for site in &self.call_sites {
            let result = self
                .nodes
                .get(&site.node)
                .map_or(Ty::Unknown, |ty| self.vars.resolve(ty));
            if !result.is_unknown() {
                continue;
            }
            let receiver = match &site.receiver {
                Some(ty) => {
                    let ty = self.vars.resolve(ty);
                    if ty.is_unknown() {
                        continue;
                    }
                    label(&ty)
                }
                None => "path".to_string(),
            };
            found.push(Untyped {
                file: self.ctx.file.to_string(),
                line: site.line,
                receiver,
                name: site.name.clone(),
            });
        }
    }
}

/// The receiver kind the method tables of this pass switch on.
fn label(ty: &Ty) -> String {
    match ty {
        Ty::Unknown => "unknown",
        Ty::Unit => "unit",
        Ty::Bool => "bool",
        Ty::Char => "char",
        Ty::Str => "str",
        Ty::Int(_) | Ty::IntVar(_) => "int",
        Ty::F32 | Ty::F64 | Ty::FloatVar(_) => "float",
        Ty::Vec(_) => "Vec",
        Ty::Set(..) => "Set",
        Ty::Map(..) => "Map",
        Ty::Option(_) => "Option",
        Ty::Result(..) => "Result",
        Ty::Tuple(_) => "tuple",
        Ty::Struct(_) => "struct",
        Ty::Enum(_) => "enum",
        Ty::Iter(_) => "Iterator",
        Ty::Range(_) => "Range",
        Ty::Closure(..) => "closure",
        Ty::Json => "Json",
        Ty::Entry(_) => "Entry",
        Ty::Generic(_) => "generic",
        Ty::Named(name, _) => return name.to_string(),
    }
    .to_string()
}
