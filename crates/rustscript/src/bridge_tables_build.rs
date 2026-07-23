//! Harvest the interpreter's supported method names straight from the bridge
//! source, at build time.
//!
//! A bridge dispatches on a method name with a `match` whose arms are string
//! literals. That set is the interpreter's real surface, but it lived only
//! inside those match arms, so nothing could read it and `rust check` had no
//! way to tell a script it calls a method the interpreter does not implement.
//!
//! This parses the bridge files with `syn`, which is exact rather than a
//! guess, and writes the names out as tables the coverage checker reads. There
//! is no second hand written list, so nothing can drift: adding an arm adds it
//! to the table on the next build, and renaming a harvested function is a hard
//! build failure rather than a silently emptied table.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use syn::visit::Visit;

/// Which engine a table belongs to. A `#[tokio::main]` script runs on the
/// parallel engine, whose surface is thinner, so the two are kept apart.
#[derive(Clone, Copy, PartialEq)]
pub enum Engine {
    Fast,
    Parallel,
    /// Present in both, for example the shared char table.
    Both,
}

impl Engine {
    fn as_str(self) -> &'static str {
        match self {
            Engine::Fast => "Engine::Fast",
            Engine::Parallel => "Engine::Parallel",
            Engine::Both => "Engine::Both",
        }
    }
}

/// A function whose dispatch arms are harvested, and the receiver those methods
/// belong to. `recv` is the type name the checker infers for a value, or "*"
/// when the arms apply to any receiver.
pub struct Bridge {
    pub file: &'static str,
    pub func: &'static str,
    pub engine: Engine,
    pub recv: &'static str,
}

pub const BRIDGES: &[Bridge] = &[
    // -- shared cores, one source materialized by both engines ------------
    b("shared.rs", "str_core", Engine::Both, "Str"),
    b("shared.rs", "color_core", Engine::Both, "Str"),
    b("shared.rs", "num_core", Engine::Both, "*"),
    b("shared.rs", "char_method", Engine::Both, "Char"),
    // -- fast engine ------------------------------------------------------
    b("methods.rs", "str_method_slow", Engine::Fast, "Str"),
    b("methods.rs", "vec_method", Engine::Fast, "Vec"),
    b("methods.rs", "map_method", Engine::Fast, "Map"),
    b("methods.rs", "opt_method", Engine::Fast, "Option"),
    b("methods.rs", "res_method", Engine::Fast, "Result"),
    b("methods.rs", "entry_method", Engine::Fast, "Entry"),
    b("builtins.rs", "builtin_method", Engine::Fast, "*"),
    b("methods.rs", "generic_method", Engine::Fast, "*"),
    b("methods.rs", "json_type_test", Engine::Fast, "*"),
    b("methods.rs", "num_method", Engine::Fast, "*"),
    b("std_bridge.rs", "path_method", Engine::Fast, "Path"),
    b("std_bridge.rs", "duration_method", Engine::Fast, "Duration"),
    b("std_bridge.rs", "metadata_method", Engine::Fast, "Metadata"),
    b(
        "std_bridge.rs",
        "os_string_method",
        Engine::Fast,
        "OsString",
    ),
    b(
        "std_bridge.rs",
        "dir_entry_method",
        Engine::Fast,
        "DirEntry",
    ),
    b(
        "std_bridge.rs",
        "file_type_method",
        Engine::Fast,
        "FileType",
    ),
    b("native.rs", "native_method", Engine::Fast, "Native"),
    b("pdf_bridge.rs", "document_method", Engine::Fast, "Document"),
    b(
        "xmltree_bridge.rs",
        "element_method",
        Engine::Fast,
        "Element",
    ),
    b("process.rs", "command_method", Engine::Fast, "Command"),
    b("regex_bridge.rs", "regex_method", Engine::Fast, "Regex"),
    b("regex_bridge.rs", "match_method", Engine::Fast, "Match"),
    b(
        "regex_bridge.rs",
        "captures_method",
        Engine::Fast,
        "Captures",
    ),
    b("iterator.rs", "iterator_method", Engine::Fast, "Iterator"),
    b(
        "iterator.rs",
        "iterator_higher_order",
        Engine::Fast,
        "Iterator",
    ),
    b(
        "iterator.rs",
        "iterator_predicate",
        Engine::Fast,
        "Iterator",
    ),
    b("http.rs", "request_method", Engine::Fast, "Request"),
    b("http.rs", "builder_method", Engine::Fast, "Builder"),
    b("http.rs", "response_method", Engine::Fast, "Response"),
    b("http.rs", "status_method", Engine::Fast, "Status"),
    b(
        "crates_bridge.rs",
        "datetime_method",
        Engine::Fast,
        "DateTime",
    ),
    b("crates_bridge.rs", "base64_method", Engine::Fast, "Base64"),
    b("crates_bridge.rs", "rng_method", Engine::Fast, "Rng"),
    b("crates_bridge.rs", "sha256_method", Engine::Fast, "Sha256"),
    b("winreg_bridge.rs", "regkey_method", Engine::Fast, "RegKey"),
    b(
        "service_bridge.rs",
        "service_method",
        Engine::Fast,
        "Service",
    ),
    b(
        "service_bridge.rs",
        "manager_method",
        Engine::Fast,
        "ServiceManager",
    ),
    b("wmi_bridge.rs", "wmi_method", Engine::Fast, "WmiConnection"),
    b(
        "process.rs",
        "exitstatus_method",
        Engine::Fast,
        "ExitStatus",
    ),
    b("std_bridge.rs", "std_stream_method", Engine::Fast, "Native"),
    b("std_bridge.rs", "openoptions_method", Engine::Fast, "OpenOptions"),
    b("http.rs", "header_map_method", Engine::Fast, "HeaderMap"),
    b(
        "http.rs",
        "header_value_method",
        Engine::Fast,
        "HeaderValue",
    ),
    b("higher_order.rs", "vec_higher_order", Engine::Fast, "Vec"),
    b(
        "higher_order.rs",
        "option_higher_order",
        Engine::Fast,
        "Option",
    ),
    b(
        "higher_order.rs",
        "result_higher_order",
        Engine::Fast,
        "Result",
    ),
    b(
        "higher_order.rs",
        "entry_higher_order",
        Engine::Fast,
        "Entry",
    ),
    // -- parallel engine --------------------------------------------------
    b("pbridge.rs", "str_method", Engine::Parallel, "Str"),
    b("pbridge.rs", "vec_method", Engine::Parallel, "Vec"),
    b("pbridge.rs", "higher_order", Engine::Parallel, "Vec"),
    b("pbridge.rs", "map_method", Engine::Parallel, "Map"),
    b("pbridge.rs", "enum_method", Engine::Parallel, "Enum"),
    b(
        "pbridge.rs",
        "duration_method",
        Engine::Parallel,
        "Duration",
    ),
    b("pbridge.rs", "scalar_method", Engine::Parallel, "*"),
    b("pprocess.rs", "command_method", Engine::Parallel, "Command"),
    b("pprocess.rs", "child_method", Engine::Parallel, "Child"),
    b("pprocess.rs", "native_method", Engine::Parallel, "Native"),
    b("pregex.rs", "regex_method", Engine::Parallel, "Regex"),
    b("pregex.rs", "match_method", Engine::Parallel, "Match"),
    b("pregex.rs", "captures_method", Engine::Parallel, "Captures"),
    b("phttp.rs", "request_method", Engine::Parallel, "Request"),
    b("phttp.rs", "client_method", Engine::Parallel, "Client"),
    b("phttp.rs", "builder_method", Engine::Parallel, "Builder"),
    b("phttp.rs", "response_method", Engine::Parallel, "Response"),
    b("phttp.rs", "status_method", Engine::Parallel, "Status"),
    b(
        "phttp.rs",
        "header_map_method",
        Engine::Parallel,
        "HeaderMap",
    ),
    b(
        "phttp.rs",
        "header_value_method",
        Engine::Parallel,
        "HeaderValue",
    ),
    b(
        "pbridge.rs",
        "exitstatus_method",
        Engine::Parallel,
        "ExitStatus",
    ),
    b("pbridge.rs", "output_method", Engine::Parallel, "Output"),
    b("pbridge.rs", "eval_method", Engine::Parallel, "*"),
];

const fn b(file: &'static str, func: &'static str, engine: Engine, recv: &'static str) -> Bridge {
    Bridge {
        file,
        func,
        engine,
        recv,
    }
}

/// Collects every string literal inside one bridge function.
///
/// Bridges do not all dispatch the same way. Most use a `match` on the method
/// name, but some use `if name == "x"` or `matches!(name, "a" | "b")`, and a
/// collector that only understood match arms reported those as unimplemented
/// when they work fine.
///
/// So this takes every string literal in the function rather than trying to
/// recognise each dispatch style. The trade is deliberate and one directional:
/// a stray literal only makes the check accept a name it should not, while
/// missing one makes it reject working code, which is far worse.
#[derive(Default)]
struct LitCollector {
    names: BTreeSet<String>,
}

impl LitCollector {
    fn take(&mut self, value: String) {
        // Method names only: no paths, spaces, or format templates.
        if !value.is_empty()
            && value
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            self.names.insert(value);
        }
    }

    /// A macro body is an unparsed token stream, so `matches!(name, "lock")`
    /// is invisible to the ast visitor. Walk the raw tokens for literals too.
    fn take_tokens(&mut self, tokens: proc_macro2::TokenStream) {
        for tree in tokens {
            match tree {
                proc_macro2::TokenTree::Literal(lit) => {
                    let text = lit.to_string();
                    if let Some(inner) = text.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
                        self.take(inner.to_string());
                    }
                }
                proc_macro2::TokenTree::Group(group) => self.take_tokens(group.stream()),
                _ => {}
            }
        }
    }
}

impl<'ast> Visit<'ast> for LitCollector {
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        self.take_tokens(mac.tokens.clone());
        syn::visit::visit_macro(self, mac);
    }

    fn visit_lit_str(&mut self, lit: &'ast syn::LitStr) {
        self.take(lit.value());
    }
}

/// Find a function by name anywhere in the file, including inside `impl`
/// blocks and inside `mod` blocks, and harvest its arms.
///
/// Names are unioned across every match rather than replaced. A bridge split
/// by `#[cfg]` declares the same function twice, once real and once as a stub
/// that bails, and replacing would let whichever copy comes last win. The stub
/// has no arms, so that emptied the table and `rust check` then rejected every
/// method the real one implements.
struct FnFinder<'a> {
    want: &'a str,
    found: Option<BTreeSet<String>>,
}

impl<'ast> Visit<'ast> for FnFinder<'_> {
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if item.sig.ident == self.want {
            let mut c = LitCollector::default();
            c.visit_block(&item.block);
            self.found.get_or_insert_with(BTreeSet::new).extend(c.names);
        }
        syn::visit::visit_item_fn(self, item);
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if item.sig.ident == self.want {
            let mut c = LitCollector::default();
            c.visit_block(&item.block);
            self.found.get_or_insert_with(BTreeSet::new).extend(c.names);
        }
        syn::visit::visit_impl_item_fn(self, item);
    }
}

/// Harvest one function's dispatch names, or `None` when it is missing.
fn harvest(dir: &Path, file: &str, func: &str) -> Option<BTreeSet<String>> {
    let text = std::fs::read_to_string(dir.join(file)).ok()?;
    let ast = syn::parse_file(&text).ok()?;
    let mut finder = FnFinder {
        want: func,
        found: None,
    };
    finder.visit_file(&ast);
    finder.found
}

/// Harvest a table that may legitimately be absent, unlike a bridge function.
fn harvest_names(dir: &Path, file: &str, func: &str) -> BTreeSet<String> {
    harvest(dir, file, func).unwrap_or_default()
}

pub fn generate(interpreter_dir: &Path) -> String {
    let mut out = String::new();
    out.push_str(
        "// Generated by build.rs from the bridge sources. Do not edit.\n\
         // See src/bridge_tables_build.rs for how and why.\n\n",
    );

    let mut rows: Vec<String> = Vec::new();
    for bridge in BRIDGES {
        let names = harvest(interpreter_dir, bridge.file, bridge.func).unwrap_or_else(|| {
            panic!(
                "bridge function `{}` not found in {}. It was renamed or moved, \
                 which would silently empty its coverage table.",
                bridge.func, bridge.file
            )
        });
        let list: Vec<String> = names.iter().map(|n| format!("{n:?}")).collect();
        rows.push(format!(
            "    BridgeTable {{ engine: {}, recv: {:?}, names: &[{}] }},",
            bridge.engine.as_str(),
            bridge.recv,
            list.join(", ")
        ));
    }

    let _ = writeln!(
        out,
        "pub const BRIDGE_TABLES: &[BridgeTable] = &[\n{}\n];\n",
        rows.join("\n")
    );

    // The hot path methods resolve through `BuiltinId`, not a string match, so
    // their names live in the resolver instead of a bridge.
    let builtin = harvest_names(interpreter_dir, "bytecode.rs", "resolve");
    let list: Vec<String> = builtin.iter().map(|n| format!("{n:?}")).collect();
    let _ = writeln!(
        out,
        "pub const BUILTIN_IDS: &[&str] = &[{}];\n",
        list.join(", ")
    );

    out
}
