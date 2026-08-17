//! Does the interpreter implement everything this script calls?
//!
//! `cargo check` answers whether a script is valid Rust. It cannot answer this,
//! because `"x".repeat(3)` is perfectly good Rust whether or not the bridge
//! implements `repeat`. Running the script answers it only for the lines that
//! actually execute, which is why a missing method inside a loop body stayed
//! hidden until the loop had real data to iterate.
//!
//! This walks the compiled bytecode instead. Every method call the VM could
//! ever make is an `Op::Method` with a name, so every one is visible without
//! executing anything, on every branch, including code that never runs.
//!
//! Known gap: only method calls are checked, not path calls like
//! `std::process::exit(1)`. A first attempt at those reported `Ok`, `Some`,
//! `Stdio::piped` and the compiler internal `::unreachable_match` as missing,
//! because path dispatch is spread across more sites than the method bridges
//! and constructors are not bridge calls at all. A noisy check is worse than no
//! check, so path calls are left out until their tables are mapped properly.
//!
//! Where the receiver type is knowable it is used, so a `Vec` calling a `String`
//! method is still caught. That includes the type the author wrote on a
//! parameter, which is a fact rather than a guess. A `serde_json::Value` is
//! checked against every shape it can be at runtime, because the value that
//! reaches the call could be any of them. That is what a name-only answer
//! missed: `get` was implemented on a map, so `rust check` passed a script
//! whose json turned out to be null, and the run then aborted on the method
//! the check had just vouched for.
//!
//! Where the type is not knowable, the check falls back to asking whether any
//! bridge implements that name at all. That direction is
//! deliberate: an unknown receiver reports nothing rather than guessing, so
//! the check never invents a problem.

use std::collections::BTreeSet;

use super::bytecode::{BuiltinId, Chunk, Const, Op};

include!(concat!(env!("OUT_DIR"), "/bridge_tables.rs"));

pub struct BridgeTable {
    pub recv: &'static str,
    pub names: &'static [&'static str],
}

/// One method the interpreter has no implementation for.
pub struct Finding {
    pub method: String,
    /// The receiver type when it could be determined, for a sharper message.
    pub recv: Option<String>,
    /// The function the call sits in.
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

/// A receiver type inferred from the op that produced the value.
#[derive(Clone, Copy, PartialEq)]
enum Ty<'a> {
    Str,
    Int,
    Float,
    Bool,
    Char,
    Vec,
    Map,
    /// A `serde_json::Value`, which is any json shape at runtime.
    Json,
    /// A user struct or enum, checked against the script's own impls.
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
            // The scalar bridges share one table, so they are checked by name
            // rather than per type.
            Ty::Int | Ty::Float | Ty::Bool | Ty::Char | Ty::Unknown => None,
        }
    }

    /// The type a written annotation names, for the parameters whose type the
    /// author spelled out. Only the shapes the tables can check are mapped,
    /// plus the script's own types; everything else stays Unknown and is
    /// checked by name alone.
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

/// The script's own method surface: `(bare type name, method)` pairs from
/// its impl blocks, and the bare type names that have any impl at all.
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
            let bare = super::resolver::bare(&ty).to_string();
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
}

/// The shapes a `serde_json::Value` can be at runtime. A method called on one
/// has to work on every shape, since the check cannot know which it will be.
/// This is what a name-only answer missed: a map has `get`, a json null is an
/// Option and did not, so `rust check` passed a script that then aborted.
const JSON_SHAPES: &[&str] = &["Map", "Vec", "Str", "Option"];

/// Every name any bridge implements. `BUILTIN_IDS` is harvested
/// from the shared id resolver, so it says a name has a fast dispatch id, not
/// which dispatch path implements it. Much of the surface dispatches by name, so
/// its tables are its whole surface and the id list must not vouch for it.
fn any_name(method: &str) -> bool {
    BUILTIN_IDS.contains(&method)
        || VM_BUILTINS.contains(&method)
        || BRIDGE_TABLES.iter().any(|t| t.names.contains(&method))
}

/// Methods the VM answers itself, before any bridge is reached, because
/// they rewrite the receiver register rather than return a value. They are
/// dispatched by `BuiltinId` rather than by name, so the harvest cannot see
/// them in the bridge tables and they are listed here instead.
/// `parse` is here for the same reason. It is answered from the turbofish
/// before name dispatch, so the name never appears in a table.
const VM_BUILTINS: &[&str] = &[
    "clone_from",
    "push",
    "push_str",
    "parse",
    "make_ascii_uppercase",
    "make_ascii_lowercase",
];

/// Whether the bridge for this receiver implements the method.
fn on_recv(recv: &str, method: &str) -> bool {
    let mut saw_table = false;
    for table in BRIDGE_TABLES {
        if table.recv == recv {
            saw_table = true;
            if table.names.contains(&method) {
                return true;
            }
        }
        // A table that applies to any receiver, and the generic methods every
        // value has, are always in play.
        if table.recv == "*" && table.names.contains(&method) {
            return true;
        }
    }
    // With no table for this receiver there is nothing to say, so defer to
    // `any_name` rather than reporting.
    if !saw_table {
        return any_name(method);
    }
    // A table exists for the receiver and the method is not in it. Methods
    // that resolve through `BuiltinId` outside the string tables carry their
    // own receiver tags, so a `Vec`-only name no longer vouches for a
    // `String` receiver.
    let tagged = BuiltinId::resolve(method).receivers();
    tagged.contains(&"*") || tagged.contains(&recv)
}

/// Methods every value carries, handled before bridge dispatch.
const UNIVERSAL: &[&str] = &["clone", "to_string"];

/// The whole bridged surface as (receiver, method), sorted by receiver then
/// method. Message literals the harvest picks up alongside the real names are
/// filtered the same way the drift test filters them.
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

/// Walk a chunk and its nested closures, reporting unimplemented methods.
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
                // A user type answers from the script's own impls, plus the
                // any-receiver bridge surface every value carries. A common
                // bridge name on the wrong receiver is exactly what a
                // name-only answer used to vouch for. A type with its own
                // `next` is an iterator, so the whole adaptor surface
                // applies to it and the check falls back to name only.
                Ty::User(ty_name) => {
                    user.has(ty_name, method)
                        || BRIDGE_TABLES
                            .iter()
                            .any(|t| t.recv == "*" && t.names.contains(&method.as_str()))
                        || (user.has(ty_name, "next") && any_name(method))
                }
                _ => match ty.name() {
                    Some(recv_name) => on_recv(recv_name, method),
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

/// The type of a register, from the nearest earlier op that wrote it. A
/// parameter register nothing has written yet is the type the author wrote in
/// the signature. Anything less direct is `Unknown`, which makes the check
/// fall back to name only rather than guess.
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
            Op::Fmt { dst, .. } if *dst == reg => return Ty::Str,
            // A freshly built user struct or enum names its type exactly.
            Op::MakeStruct { dst, info, .. } if *dst == reg => {
                return Ty::User(&chunk.struct_lits[*info as usize].shape.name);
            }
            Op::MakeEnum { dst, info, .. } | Op::LoadEnum { dst, info } if *dst == reg => {
                return Ty::User(&chunk.enum_variants[*info as usize].enum_name);
            }
            // A one-segment path value naming a script type is a unit
            // struct, `let d = Dog;`, so the report can name the type.
            Op::PathValue { dst, path } if *dst == reg => {
                let segs = &chunk.paths[*path as usize].0;
                if let [name] = segs.as_slice()
                    && user.types.contains(name)
                {
                    return Ty::User(name);
                }
                return Ty::Unknown;
            }
            // Any other write to this register loses the trail.
            _ => {
                if writes(op) == Some(reg) {
                    return Ty::Unknown;
                }
            }
        }
    }
    // Nothing wrote it, so a register inside the parameter block still holds
    // the argument, whose type is written down in the signature.
    match chunk.param_types.get(reg as usize) {
        Some(Some(name)) => Ty::from_annotation(name, user),
        _ => Ty::Unknown,
    }
}

/// The register an op writes, when it has a single obvious destination.
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
        | Op::LoadGlobal { dst, .. }
        | Op::LoadUpvalue { dst, .. }
        | Op::LoadCell { dst, .. }
        | Op::Index { dst, .. }
        | Op::Deref { dst, .. }
        | Op::GetField { dst, .. } => Some(*dst),
        _ => None,
    }
}

/// Report every method the interpreter does not implement, across every
/// function of the program, executed or not.
pub fn report(
    functions: &[std::sync::Arc<Chunk>],
    methods: impl Iterator<Item = (String, String)>,
) -> Vec<Finding> {
    let user = UserMethods::new(methods);
    let mut out = Vec::new();
    for chunk in functions {
        walk(chunk, &user, &mut out);
    }
    // One report per distinct method, so a helper called in a loop does not
    // print the same line many times.
    let mut seen = BTreeSet::new();
    out.retain(|f| seen.insert((f.method.clone(), f.recv.clone())));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Method names the tables carry, message literals filtered out. The
    /// harvest keeps every string literal in a bridge function, so error
    /// texts with spaces or backticks ride along and must not count as names.
    fn table_names() -> BTreeSet<&'static str> {
        BRIDGE_TABLES
            .iter()
            .flat_map(|t| t.names.iter().copied())
            .filter(|n| !n.contains(' ') && !n.contains('`') && n.len() > 1)
            .collect()
    }

    /// The closure-taking and id-resolved methods must stay visible to
    /// `rust check`, whether they live in a string table or the id list.
    #[test]
    fn the_higher_order_surface_is_known() {
        for method in ["sort_by_key", "retain", "fold", "map_err", "reduce"] {
            assert!(any_name(method), "`{method}` must be known to the checker");
        }
        assert!(on_recv("Vec", "sort_by_key"));
        // The VM answers these itself, so they stay known even though no
        // bridge table can name them.
        for method in VM_BUILTINS {
            assert!(on_recv("Str", method));
            assert!(any_name(method));
        }
        assert!(!table_names().is_empty());
    }

    /// A json value can be any shape at runtime, so a method must exist on
    /// every shape to pass. `get` exists on a map but not on a null.
    #[test]
    fn a_json_method_needs_every_shape() {
        assert!(JSON_SHAPES.iter().all(|shape| on_recv(shape, "clone")));
    }
}
