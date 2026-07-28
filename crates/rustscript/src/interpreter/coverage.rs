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

/// Every name any bridge in this engine implements. `BUILTIN_IDS` is harvested
/// from the shared id resolver, so it says a name has a fast dispatch id, not
/// which engine implements it. The parallel engine dispatches by name only, so
/// its tables are its whole surface and the id list must not vouch for it.
fn any_name(engine: Engine, method: &str) -> bool {
    (engine == Engine::Fast && BUILTIN_IDS.contains(&method))
        || VM_BUILTINS.contains(&method)
        || BRIDGE_TABLES
            .iter()
            .any(|t| applies(t, engine) && t.names.contains(&method))
}

/// Methods both VMs answer themselves, before any bridge is reached, because
/// they rewrite the receiver register rather than return a value. They are
/// dispatched by `BuiltinId` rather than by name, so the harvest cannot see
/// them in either engine's tables and they are listed here instead.
const VM_BUILTINS: &[&str] = &["clone_from", "push", "push_str"];

/// Whether the bridge for this receiver implements the method.
fn on_recv(engine: Engine, recv: &str, method: &str) -> bool {
    if VM_BUILTINS.contains(&method) {
        return true;
    }
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
    // This engine has a table for the receiver and the method is not in it.
    //
    // `BUILTIN_IDS` used to rescue it here, but that list is harvested from
    // the shared id resolver and says nothing about which engine implements a
    // name. The parallel engine dispatches entirely by name, so its tables are
    // its real surface, and letting an id rescue a miss made `rust check`
    // answer ok for `#[tokio::main]` scripts calling `sort_by_key`, `fold`,
    // `retain` or `map_err`, which then aborted at runtime. The fast engine
    // does resolve many of these through the id path outside the string
    // tables, so the rescue still stands there.
    engine == Engine::Fast && BUILTIN_IDS.contains(&method)
}

/// Methods every value carries, handled before bridge dispatch.
const UNIVERSAL: &[&str] = &["clone", "to_string"];

/// Which engines carry one bridged method.
#[derive(Clone, Copy, PartialEq)]
pub enum Avail {
    Both,
    FastOnly,
    ParallelOnly,
}

/// The whole bridged surface as (receiver, method, availability), sorted by
/// receiver then method. Message literals the harvest picks up alongside the
/// real names are filtered the same way the drift test filters them.
pub fn surface() -> Vec<(&'static str, &'static str, Avail)> {
    let mut merged: std::collections::BTreeMap<(&str, &str), (bool, bool)> =
        std::collections::BTreeMap::new();
    for table in BRIDGE_TABLES {
        for name in table.names {
            if name.contains(' ') || name.contains('`') || name.len() <= 1 {
                continue;
            }
            let entry = merged.entry((table.recv, name)).or_insert((false, false));
            if applies(table, Engine::Fast) {
                entry.0 = true;
            }
            if applies(table, Engine::Parallel) {
                entry.1 = true;
            }
        }
    }
    for name in BUILTIN_IDS {
        if name.len() > 1 {
            merged.insert(("builtin", name), (true, true));
        }
    }
    merged
        .into_iter()
        .map(|((recv, name), (fast, parallel))| {
            let avail = match (fast, parallel) {
                (true, true) => Avail::Both,
                (true, false) => Avail::FastOnly,
                _ => Avail::ParallelOnly,
            };
            (recv, name, avail)
        })
        .collect()
}

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
                    Const::Float(_) | Const::F32(_) => Ty::Float,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Method names one engine's tables carry, message literals filtered out.
    /// The harvest keeps every string literal in a bridge function, so error
    /// texts with spaces or backticks ride along and must not count as names.
    fn engine_names(engine: Engine) -> BTreeSet<&'static str> {
        BRIDGE_TABLES
            .iter()
            .filter(|t| applies(t, engine))
            .flat_map(|t| t.names.iter().copied())
            .filter(|n| !n.contains(' ') && !n.contains('`') && n.len() > 1)
            .collect()
    }

    /// A method the fast engine reaches through its builtin id table but the
    /// parallel engine does not implement must be reported for the parallel
    /// engine. `BUILTIN_IDS` used to vouch for both engines, so `rust check`
    /// answered ok for a `#[tokio::main]` script calling `sort_by_key`, which
    /// then aborted at runtime with the exact message the check exists to
    /// deliver ahead of time.
    #[test]
    fn a_fast_only_builtin_is_reported_for_the_parallel_engine() {
        for method in ["sort_by_key", "retain", "fold", "map_err", "reduce"] {
            assert!(
                any_name(Engine::Fast, method),
                "`{method}` must stay available on the fast engine"
            );
            assert!(
                !any_name(Engine::Parallel, method),
                "`{method}` is not implemented in tokio mode and must be reported"
            );
        }
        assert!(on_recv(Engine::Fast, "Vec", "sort_by_key"));
        assert!(!on_recv(Engine::Parallel, "Vec", "sort_by_key"));
        // The VM answers these itself on both engines, so they stay known even
        // though no bridge table can name them.
        for method in VM_BUILTINS {
            assert!(on_recv(Engine::Parallel, "Str", method));
            assert!(any_name(Engine::Parallel, method));
        }
    }

    /// Methods the parallel engine really does implement must not be reported.
    /// These come from tables the harvest was not reading, and the engine
    /// agnostic id list was the only thing keeping working scripts from being
    /// rejected once that rescue was removed.
    #[test]
    fn parallel_methods_outside_the_id_table_are_not_reported() {
        for method in ["and_then", "is_some_and", "elapsed", "map_or", "or_else"] {
            assert!(
                any_name(Engine::Parallel, method),
                "`{method}` runs in tokio mode and must not be reported"
            );
        }
    }

    /// Every method the fast engine bridges and the parallel engine lacks.
    /// The gap may only shrink, or grow by a conscious entry in `KNOWN_GAP`.
    /// A new fast-only method fails this test, so drift between the engines
    /// is a decision, never an accident.
    #[test]
    fn parallel_engine_gap_is_deliberate() {
        let fast = engine_names(Engine::Fast);
        let parallel = engine_names(Engine::Parallel);
        let gap: BTreeSet<&str> = fast.difference(&parallel).copied().collect();
        let known: BTreeSet<&str> = KNOWN_GAP.iter().copied().collect();
        let new: Vec<&&str> = gap.difference(&known).collect();
        let closed: Vec<&&str> = known.difference(&gap).collect();
        assert!(
            new.is_empty(),
            "new fast-only methods. Port them to the parallel engine, or add \
             them to KNOWN_GAP as a deliberate exclusion: {new:?}"
        );
        assert!(
            closed.is_empty(),
            "methods no longer fast-only, remove them from KNOWN_GAP: {closed:?}"
        );
    }

    /// The tracked fast-only surface, sorted. Shrinking it is progress.
    const KNOWN_GAP: &[&str] = &[
        "accept",
        "access",
        "accessed",
        "account_name",
        "ancestors",
        "and_modify",
        "append",
        "as_deref_mut",
        "as_os_str",
        "as_path",
        "chain_update",
        "change_config",
        "change_page_content",
        "close",
        "connect",
        "create_subkey",
        "create",
        "create_new",
        "created",
        "current_state",
        "cwd",
        "decode",
        "dedup",
        "delete_subkey",
        "delete_subkey_all",
        "delete_value",
        "dependencies",
        "dev",
        "display",
        "display_name",
        "drain",
        "duration_since",
        "encode",
        "enum_keys",
        "enum_values",
        "err",
        "error_control",
        "executable_path",
        "exists",
        "extension",
        "file_name",
        "file_stem",
        "file_type",
        "fill",
        "fill_bytes",
        "finalize",
        "flags",
        "flatten",
        "fold",
        "gen",
        "gen_bool",
        "gen_range",
        "get_all",
        "get_page_content",
        "get_pages",
        "get_raw_value",
        "get_text",
        "get_value",
        "gid",
        "incoming",
        "inner",
        "ino",
        "into_os_string",
        "is_absolute",
        "is_dir",
        "is_err_and",
        "is_file",
        "is_ok_and",
        "is_symlink",
        "key",
        "local_addr",
        "manager_access",
        "map_err",
        "max_by_key",
        "metadata",
        "min_by_key",
        "mode",
        "modified",
        "mtime",
        "namespace",
        "ok_or",
        "open",
        "open_service",
        "open_subkey",
        "open_subkey_with_flags",
        "or",
        "or_default",
        "or_insert",
        "or_insert_with",
        "or_insert_with_key",
        "parent",
        "partition",
        "path",
        "peek",
        "peekable",
        "peer_addr",
        "permissions",
        "query_config",
        "query_status",
        "random",
        "random_bool",
        "random_range",
        "raw_query",
        "read",
        "read_to_end",
        "readonly",
        "redirect",
        "reduce",
        "retain",
        "reverse",
        "root",
        "save",
        "seek",
        "send_to",
        "service_type",
        "set_broadcast",
        "set_len",
        "set_modified",
        "set_raw_value",
        "set_readonly",
        "set_value",
        "shutdown",
        "skip_while",
        "sort_by",
        "sort_by_cached_key",
        "sort_by_key",
        "standard_no_pad",
        "start_type",
        "stop",
        "sync_all",
        "sync_data",
        "take_while",
        "then_some",
        "to_path_buf",
        "to_string_lossy",
        "truncate",
        "try_clone",
        "try_wait",
        "uid",
        "unwrap_err",
        "update",
        "url_safe",
        "url_safe_no_pad",
        "values_mut",
        "with_extension",
    ];
}
