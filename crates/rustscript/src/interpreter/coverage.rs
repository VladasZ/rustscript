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
//! method is still caught. Where it is not, the check falls back to asking
//! whether any bridge in this engine implements that name at all. That
//! direction is deliberate: an unknown receiver reports nothing rather than
//! guessing, so the check never invents a problem.

use std::collections::BTreeSet;

use super::bytecode::{Chunk, Const, Op};

include!(concat!(env!("OUT_DIR"), "/bridge_tables.rs"));

#[derive(Clone, Copy, PartialEq)]
pub enum Engine {
    Fast,
    Parallel,
    Both,
}

pub struct BridgeTable {
    pub engine: Engine,
    pub recv: &'static str,
    pub names: &'static [&'static str],
}

/// One method the interpreter has no implementation for.
pub struct Finding {
    pub method: String,
    /// The receiver type when it could be determined, for a sharper message.
    pub recv: Option<&'static str>,
    /// The function the call sits in.
    pub func: String,
}

impl Finding {
    pub fn message(&self) -> String {
        match self.recv {
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
enum Ty {
    Str,
    Int,
    Float,
    Bool,
    Char,
    Vec,
    Unknown,
}

impl Ty {
    fn name(self) -> Option<&'static str> {
        match self {
            Ty::Str => Some("Str"),
            Ty::Vec => Some("Vec"),
            // The scalar bridges share one table, so they are checked by name
            // rather than per type.
            Ty::Int | Ty::Float | Ty::Bool | Ty::Char | Ty::Unknown => None,
        }
    }
}

/// Whether a table applies to the engine being checked.
fn applies(table: &BridgeTable, engine: Engine) -> bool {
    table.engine == engine || table.engine == Engine::Both
}

/// Every name any bridge in this engine implements.
fn any_name(engine: Engine, method: &str) -> bool {
    BUILTIN_IDS.contains(&method)
        || BRIDGE_TABLES
            .iter()
            .any(|t| applies(t, engine) && t.names.contains(&method))
}

/// Whether the bridge for this receiver implements the method.
fn on_recv(engine: Engine, recv: &str, method: &str) -> bool {
    let mut saw_table = false;
    for table in BRIDGE_TABLES.iter().filter(|t| applies(t, engine)) {
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
    // With no table for this receiver there is nothing to say, so defer to the
    // engine wide answer rather than reporting.
    if !saw_table {
        return any_name(engine, method);
    }
    BUILTIN_IDS.contains(&method)
}

/// Methods every value carries, handled before bridge dispatch.
const UNIVERSAL: &[&str] = &["clone", "to_string"];

/// Walk a chunk and its nested closures, reporting unimplemented methods.
fn walk(chunk: &Chunk, engine: Engine, user: &BTreeSet<String>, out: &mut Vec<Finding>) {
    for (index, op) in chunk.code.iter().enumerate() {
        if let Op::Method { recv, name, .. } = op {
            let method = &chunk.names[*name as usize].text;
            if UNIVERSAL.contains(&method.as_str()) || user.contains(method) {
                continue;
            }
            let ty = infer(chunk, index, *recv);
            let known = match ty.name() {
                Some(recv_name) => on_recv(engine, recv_name, method),
                None => any_name(engine, method),
            };
            if !known {
                out.push(Finding {
                    method: method.clone(),
                    recv: ty.name(),
                    func: chunk.name.clone(),
                });
            }
        }
    }
    for child in &chunk.children {
        walk(child, engine, user, out);
    }
}

/// The type of a register, from the nearest earlier op that wrote it. Anything
/// less direct is `Unknown`, which makes the check fall back to name only
/// rather than guess.
fn infer(chunk: &Chunk, before: usize, reg: u16) -> Ty {
    for op in chunk.code[..before].iter().rev() {
        match op {
            Op::LoadConst { dst, k } if *dst == reg => {
                return match chunk.consts[*k as usize] {
                    Const::Str(_) => Ty::Str,
                    Const::Char(_) => Ty::Char,
                    Const::Float(_) => Ty::Float,
                    Const::Bytes(_) => Ty::Vec,
                };
            }
            Op::LoadInt { dst, .. } if *dst == reg => return Ty::Int,
            Op::LoadBool { dst, .. } if *dst == reg => return Ty::Bool,
            Op::MakeVec { dst, .. } if *dst == reg => return Ty::Vec,
            Op::Fmt { dst, .. } if *dst == reg => return Ty::Str,
            // Any other write to this register loses the trail.
            _ => {
                if writes(op) == Some(reg) {
                    return Ty::Unknown;
                }
            }
        }
    }
    Ty::Unknown
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
        | Op::Index { dst, .. }
        | Op::GetField { dst, .. } => Some(*dst),
        _ => None,
    }
}

/// Report every method the interpreter does not implement, across every
/// function of the program, executed or not.
pub fn report(
    functions: &[std::rc::Rc<Chunk>],
    methods: impl Iterator<Item = String>,
    engine: Engine,
) -> Vec<Finding> {
    let user: BTreeSet<String> = methods.collect();
    let mut out = Vec::new();
    for chunk in functions {
        walk(chunk, engine, &user, &mut out);
    }
    // One report per distinct method, so a helper called in a loop does not
    // print the same line many times.
    let mut seen = BTreeSet::new();
    out.retain(|f| seen.insert((f.method.clone(), f.recv)));
    out
}
