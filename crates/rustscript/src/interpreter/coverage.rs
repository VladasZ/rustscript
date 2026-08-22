//! Does the interpreter implement everything this script calls? `cargo
//! check` cannot answer this and running only proves the lines that run, so
//! this walks the compiled bytecode, where every method call is visible on
//! every branch.
//!
//! Known gap, path calls like `std::process::exit(1)` are not checked. A
//! first attempt reported constructors as missing, and a noisy check is
//! worse than none.
//!
//! Where the receiver type is knowable it is used. A `serde_json::Value` is
//! checked against every shape it can be, a name only answer once passed a
//! `get` that aborted on a null. An unknown receiver reports nothing rather
//! than guessing.

use std::collections::BTreeSet;

use super::bytecode::{BuiltinId, Chunk, Const, Op};

include!(concat!(env!("OUT_DIR"), "/bridge_tables.rs"));

pub struct BridgeTable {
    pub recv: &'static str,
    pub names: &'static [&'static str],
}

pub struct Finding {
    pub method: String,
    /// For a sharper message.
    pub recv: Option<String>,
    pub func: String,
}

impl Finding {
    pub fn message(&self) -> String {
        match &self.recv {
            Some(recv) => format!(
                "`{}` on {} is not implemented by the interpreter, in `{}`",
                self.method, recv, self.func
            ),
            None => format!(
                "`{}` is not implemented by the interpreter, in `{}`",
                self.method, self.func
            ),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Ty<'a> {
    Str,
    Int,
    Float,
    Bool,
    Char,
    Vec,
    Map,
    /// Any json shape at runtime.
    Json,
    /// Checked against the script's own impls.
    User(&'a str),
    Unknown,
}

impl<'a> Ty<'a> {
    fn name(self) -> Option<&'a str> {
        match self {
            Ty::Str => Some("Str"),
            Ty::Vec => Some("Vec"),
            Ty::Map => Some("Map"),
            Ty::Json => Some("Value"),
            Ty::User(name) => Some(name),
            // The scalar bridges share one table, so they are checked by
            // name.
            Ty::Int | Ty::Float | Ty::Bool | Ty::Char | Ty::Unknown => None,
        }
    }

    /// Only the shapes the tables can check are mapped, the rest stays
    /// `Unknown`.
    fn from_annotation(name: &'a str, user: &UserMethods) -> Ty<'a> {
        match name {
            "Value" => Ty::Json,
            "String" | "str" => Ty::Str,
            "Vec" | "VecDeque" => Ty::Vec,
            "HashMap" | "BTreeMap" | "IndexMap" => Ty::Map,
            other if user.types.contains(other) => Ty::User(other),
            _ => Ty::Unknown,
        }
    }
}

/// `(bare type name, method)` pairs from the script's impl blocks.
pub struct UserMethods {
    pairs: BTreeSet<(String, String)>,
    types: BTreeSet<String>,
    names: BTreeSet<String>,
}

impl UserMethods {
    pub fn new(methods: impl Iterator<Item = (String, String)>) -> Self {
        let mut pairs = BTreeSet::new();
        let mut types = BTreeSet::new();
        let mut names = BTreeSet::new();
        for (ty, method) in methods {
            // `impl T for Vec<u8>` is keyed with generics the checker's shapes
            // do not carry.
            let bare = super::resolver::bare(&ty);
            let bare = bare.split('<').next().unwrap_or(bare).to_string();
            types.insert(bare.clone());
            names.insert(method.clone());
            pairs.insert((bare, method));
        }
        Self {
            pairs,
            types,
            names,
        }
    }

    fn has(&self, ty: &str, method: &str) -> bool {
        self.pairs
            .contains(&(super::resolver::bare(ty).to_string(), method.to_string()))
    }

    /// `Str` stands for `String`, `Map` for either map, `Vec` for either
    /// sequence.
    fn has_on_builtin(&self, recv: &str, method: &str) -> bool {
        let names: &[&str] = match recv {
            "Str" => &["String", "str"],
            "Map" => &["HashMap", "BTreeMap"],
            "Vec" => &["Vec", "VecDeque"],
            other => return self.has(other, method),
        };
        names.iter().any(|name| self.has(name, method))
    }
}

/// A method on a `serde_json::Value` must work on every shape, a map has
/// `get` and a json null did not.
const JSON_SHAPES: &[&str] = &["Map", "Vec", "Str", "Option"];

/// `BUILTIN_IDS` only says a name has a dispatch id, not which path
/// implements it, so it must not vouch for the name tables.
fn any_name(method: &str) -> bool {
    BUILTIN_IDS.contains(&method)
        || VM_BUILTINS.contains(&method)
        || BRIDGE_TABLES.iter().any(|t| t.names.contains(&method))
}

/// Methods the VM answers itself by `BuiltinId`, so the harvest cannot see
/// them. `parse` is answered from the turbofish before name dispatch.
const VM_BUILTINS: &[&str] = &[
    "clone_from",
    "push",
    "push_str",
    "parse",
    "make_ascii_uppercase",
    "make_ascii_lowercase",
];

fn on_recv(recv: &str, method: &str) -> bool {
    let mut saw_table = false;
    for table in BRIDGE_TABLES {
        if table.recv == recv {
            saw_table = true;
            if table.names.contains(&method) {
                return true;
            }
        }
        // Any receiver tables are always in play.
        if table.recv == "*" && table.names.contains(&method) {
            return true;
        }
    }
    // No table for this receiver, defer to `any_name`.
    if !saw_table {
        return any_name(method);
    }
    // `BuiltinId` methods carry their own receiver tags, so a `Vec` only name
    // does not vouch for a `String`.
    let tagged = BuiltinId::resolve(method).receivers();
    tagged.contains(&"*") || tagged.contains(&recv)
}

const UNIVERSAL: &[&str] = &["clone", "to_string"];

/// Message literals the harvest picks up are filtered like the drift test
/// filters them.
pub fn surface() -> Vec<(&'static str, &'static str)> {
    let mut merged: std::collections::BTreeSet<(&str, &str)> = std::collections::BTreeSet::new();
    for table in BRIDGE_TABLES {
        for name in table.names {
            if name.contains(' ') || name.contains('`') || name.len() <= 1 {
                continue;
            }
            merged.insert((table.recv, name));
        }
    }
    for name in BUILTIN_IDS {
        if name.len() > 1 {
            merged.insert(("builtin", name));
        }
    }
    merged.into_iter().collect()
}

fn walk(chunk: &Chunk, user: &UserMethods, out: &mut Vec<Finding>) {
    for (index, op) in chunk.code.iter().enumerate() {
        if let Op::Method { recv, name, .. } = op {
            let method = &chunk.names[*name as usize].text;
            if UNIVERSAL.contains(&method.as_str()) {
                continue;
            }
            let ty = infer(chunk, index, *recv, user);
            let known = match ty {
                Ty::Json => JSON_SHAPES.iter().all(|shape| on_recv(shape, method)),
                // A user type answers from its own impls plus the any receiver
                // surface. A type with its own `next` is an iterator, so the
                // check falls back to name only.
                Ty::User(ty_name) => {
                    user.has(ty_name, method)
                        || BRIDGE_TABLES
                            .iter()
                            .any(|t| t.recv == "*" && t.names.contains(&method.as_str()))
                        || (user.has(ty_name, "next") && any_name(method))
                }
                _ => match ty.name() {
                    Some(recv_name) => {
                        on_recv(recv_name, method) || user.has_on_builtin(recv_name, method)
                    }
                    None => user.names.contains(method) || any_name(method),
                },
            };
            if !known {
                out.push(Finding {
                    method: method.clone(),
                    recv: ty.name().map(str::to_string),
                    func: chunk.name.clone(),
                });
            }
        }
    }
    for child in &chunk.children {
        walk(child, user, out);
    }
}

/// From the nearest earlier write, or the signature for an untouched
/// parameter. Anything less direct is `Unknown`.
fn infer<'a>(chunk: &'a Chunk, before: usize, reg: u16, user: &UserMethods) -> Ty<'a> {
    for op in chunk.code[..before].iter().rev() {
        match op {
            Op::LoadConst { dst, k } if *dst == reg => {
                return match chunk.consts[*k as usize] {
                    Const::Str(_) => Ty::Str,
                    Const::Char(_) => Ty::Char,
                    Const::Float(_) | Const::F32(_) => Ty::Float,
                    Const::Bytes(_) => Ty::Vec,
                    Const::Big(..) => Ty::Int,
                };
            }
            Op::LoadInt { dst, .. } if *dst == reg => return Ty::Int,
            Op::LoadBool { dst, .. } if *dst == reg => return Ty::Bool,
            Op::MakeVec { dst, .. } if *dst == reg => return Ty::Vec,
            Op::MakeMap { dst, .. } if *dst == reg => return Ty::Map,
            Op::Fmt { dst, .. } if *dst == reg => return Ty::Str,
            Op::MakeStruct { dst, info, .. } if *dst == reg => {
                return Ty::User(&chunk.struct_lits[*info as usize].shape.name);
            }
            Op::MakeEnum { dst, info, .. } | Op::LoadEnum { dst, info } if *dst == reg => {
                let def = &chunk.enum_variants[*info as usize].def;
                return if def.user {
                    Ty::User(&def.name)
                } else {
                    Ty::Unknown
                };
            }
            // `let d = Dog;` is a unit struct.
            Op::PathValue { dst, path } if *dst == reg => {
                let segs = &chunk.paths[*path as usize].segs;
                if let [name] = segs.as_slice()
                    && user.types.contains(name)
                {
                    return Ty::User(name);
                }
                return Ty::Unknown;
            }
            _ => {
                if writes(op) == Some(reg) {
                    return Ty::Unknown;
                }
            }
        }
    }
    // Nothing wrote it, so a parameter register holds the argument.
    match chunk.param_types.get(reg as usize) {
        Some(Some(name)) => Ty::from_annotation(name, user),
        _ => Ty::Unknown,
    }
}

fn writes(op: &Op) -> Option<u16> {
    match op {
        Op::Move { dst, .. }
        | Op::Bin { dst, .. }
        | Op::Un { dst, .. }
        | Op::Method { dst, .. }
        | Op::CallFn { dst, .. }
        | Op::CallPath { dst, .. }
        | Op::CallValue { dst, .. }
        | Op::MakeStruct { dst, .. }
        | Op::MakeEnum { dst, .. }
        | Op::MakeMap { dst, .. }
        | Op::LoadGlobal { dst, .. }
        | Op::LoadUpvalue { dst, .. }
        | Op::LoadCell { dst, .. }
        | Op::Index { dst, .. }
        | Op::Deref { dst, .. }
        | Op::GetField { dst, .. } => Some(*dst),
        _ => None,
    }
}

pub fn report(
    functions: &[std::sync::Arc<Chunk>],
    methods: impl Iterator<Item = (String, String)>,
) -> Vec<Finding> {
    let user = UserMethods::new(methods);
    let mut out = Vec::new();
    for chunk in functions {
        walk(chunk, &user, &mut out);
    }
    // One report per distinct method.
    let mut seen = BTreeSet::new();
    out.retain(|f| seen.insert((f.method.clone(), f.recv.clone())));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The harvest keeps every string literal, so error texts ride along and
    /// must not count as names.
    fn table_names() -> BTreeSet<&'static str> {
        BRIDGE_TABLES
            .iter()
            .flat_map(|t| t.names.iter().copied())
            .filter(|n| !n.contains(' ') && !n.contains('`') && n.len() > 1)
            .collect()
    }

    /// The closure taking and id resolved methods must stay visible.
    #[test]
    fn the_higher_order_surface_is_known() {
        for method in ["sort_by_key", "retain", "fold", "map_err", "reduce"] {
            assert!(any_name(method), "`{method}` must be known to the checker");
        }
        assert!(on_recv("Vec", "sort_by_key"));
        // The VM answers these itself.
        for method in VM_BUILTINS {
            assert!(on_recv("Str", method));
            assert!(any_name(method));
        }
        assert!(!table_names().is_empty());
    }

    /// `get` exists on a map but not on a null.
    #[test]
    fn a_json_method_needs_every_shape() {
        assert!(JSON_SHAPES.iter().all(|shape| on_recv(shape, "clone")));
    }
}
