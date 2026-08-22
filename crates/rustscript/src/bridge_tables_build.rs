//! Collects the bridged method names from the bridge sources at build time with `syn`.
//! So the coverage checker always has the real list. Renaming a harvested function breaks the build.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use syn::visit::Visit;

use crate::builtin_id_build::{MethodRow, camel};

/// Variant name to method name, `SplitFirst` to `split_first`.
type Variants = BTreeMap<String, String>;

/// `recv` is the receiver type the checker infers, "*" means any.
pub struct Bridge {
    pub file: &'static str,
    pub func: &'static str,
    pub recv: &'static str,
}

pub const BRIDGES: &[Bridge] = &[
    // shared method cores
    b("shared.rs", "str_core", "Str"),
    b("shared.rs", "color_core", "Str"),
    b("shared.rs", "num_core", "*"),
    b("shared.rs", "float_extra", "*"),
    b("shared.rs", "char_method", "Char"),
    b("shared.rs", "regex_core", "Regex"),
    b("shared.rs", "match_core", "Match"),
    b("shared.rs", "captures_core", "Captures"),
    b("shared.rs", "duration_core", "Duration"),
    b("shared.rs", "datetime_core", "DateTime"),
    b("shared.rs", "status_core", "Status"),
    b("shared.rs", "header_value_core", "HeaderValue"),
    b("shared.rs", "exit_status_core", "ExitStatus"),
    b("shared.rs", "json_type_test", "*"),
    b("int_methods.rs", "int_method", "*"),
    // `int_method` only routes, so every family has to be here.
    b("int_methods.rs", "int_arith_method", "*"),
    b("int_methods.rs", "int_checked_family", "*"),
    b("int_methods.rs", "int_wrapping_family", "*"),
    b("int_methods.rs", "int_range_family", "*"),
    b("int_methods.rs", "int_bit_family", "*"),
    b("int_methods.rs", "int_query_method", "*"),
    b("int_methods/big.rs", "big_int_method", "*"),
    // value methods
    b("methods.rs", "json_value_method", "*"),
    b("methods.rs", "str_method", "Str"),
    b("methods.rs", "str_method_slow", "Str"),
    b("methods.rs", "opt_method", "Option"),
    b("methods.rs", "res_method", "Result"),
    b("methods.rs", "entry_method", "Entry"),
    b("methods.rs", "ordering_method", "Ordering"),
    b("cell.rs", "cell_method", "Cell"),
    b("native_methods.rs", "io_error_method", "Native"),
    b("native_methods.rs", "joinerr_method", "Native"),
    b("methods.rs", "generic_method", "*"),
    b("vecmap.rs", "vec_method", "Vec"),
    b("vecmap.rs", "deque_method", "Vec"),
    b("vecmap.rs", "vec_get", "Vec"),
    b("vecmap.rs", "edge_element_ref", "Vec"),
    b("vecmap.rs", "vec_method_by_name", "Vec"),
    b("vecmap.rs", "vec_copy_from_slice", "Vec"),
    b("vecmap.rs", "vec_min_max", "Vec"),
    b("vecmap.rs", "map_method", "Map"),
    b("higher_order.rs", "higher_order", "*"),
    b("higher_order.rs", "vec_higher_order", "Vec"),
    b("higher_order.rs", "vec_transform_ho", "Vec"),
    b("higher_order.rs", "vec_reduce_ho", "Vec"),
    b("higher_order.rs", "vec_order_ho", "Vec"),
    b("higher_order.rs", "option_higher_order", "Option"),
    b("higher_order.rs", "result_higher_order", "Result"),
    b("higher_order.rs", "entry_higher_order", "Entry"),
    b("iterator/drive.rs", "iterator_method", "Iterator"),
    b("iterator/reduce.rs", "iterator_higher_order", "Iterator"),
    b("iterator/reduce.rs", "iterator_predicate", "Iterator"),
    // dispatch front door
    b("bridge.rs", "eval_method", "*"),
    b("bridge.rs", "any_receiver_method", "*"),
    b("bridge.rs", "image_args", "*"),
    b("bridge.rs", "deref_receiver", "*"),
    b("bridge.rs", "method_by_receiver", "*"),
    b("bridge.rs", "bridge_struct_method", "*"),
    b("bridge/scalar_dispatch.rs", "scalar_method", "*"),
    b("bridge/path_calls.rs", "range_builtin", "*"),
    b("bridge.rs", "native_method", "Native"),
    b("bridge/path_calls.rs", "exitstatus_method", "ExitStatus"),
    b("bridge/path_calls.rs", "output_method", "Output"),
    b("bridge/path_calls.rs", "duration_method", "Duration"),
    b("bridge/path_calls.rs", "datetime_method", "DateTime"),
    // std
    b("std_bridge.rs", "path_method", "Path"),
    b("std_bridge.rs", "metadata_method", "Metadata"),
    b("std_bridge.rs", "os_string_method", "OsString"),
    b("std_bridge.rs", "dir_entry_method", "DirEntry"),
    b("std_bridge.rs", "file_type_method", "FileType"),
    b("std_bridge.rs", "std_stream_method", "Native"),
    b("std_bridge.rs", "openoptions_method", "OpenOptions"),
    // native handles
    b("native_methods.rs", "reader_native_method", "Native"),
    b("native_methods.rs", "writer_native_method", "Native"),
    b("native_methods.rs", "file_native_method", "Native"),
    b("native_methods.rs", "child_native_method", "Native"),
    b("native_methods.rs", "net_native_method", "Native"),
    b("native_methods.rs", "udp_native_method", "Native"),
    b("native_methods.rs", "time_native_method", "Native"),
    b("native_methods.rs", "temp_native_method", "Native"),
    // processes, regex, http
    b("process.rs", "command_method", "Command"),
    b("process.rs", "child_method", "Child"),
    // Only the lazy `find_iter` and `captures_iter` arms live here, the rest is in the shared
    // regex cores.
    b("regex_bridge.rs", "regex_method", "Regex"),
    b("http.rs", "request_method", "Request"),
    b("http.rs", "client_method", "Client"),
    b("http.rs", "builder_method", "Builder"),
    b("http.rs", "response_method", "Response"),
    b("http.rs", "header_map_method", "HeaderMap"),
    // crates
    b("crates_bridge.rs", "base64_method", "Base64"),
    b("crates_bridge.rs", "rng_method", "Rng"),
    b("crates_bridge.rs", "sha256_method", "Sha256"),
    b("pdf_bridge.rs", "document_method", "Document"),
    b("xmltree_bridge.rs", "element_method", "Element"),
    b("ratatui_render.rs", "style_method", "Style"),
    b("ratatui_render.rs", "modifier_method", "Modifier"),
    b("ratatui_render.rs", "span_method", "Span"),
    b("ratatui_render.rs", "line_method", "Line"),
    b("ratatui_render.rs", "cell_method", "Cell"),
    b("ratatui_render.rs", "row_method", "Row"),
    b("ratatui_render.rs", "table_method", "Table"),
    b("ratatui_render.rs", "block_method", "Block"),
    b("ratatui_render.rs", "sparkline_method", "Sparkline"),
    b("ratatui_render.rs", "buffer_method", "Buffer"),
    b("ratatui_render.rs", "buffer_cell_method", "BufferCell"),
    b("winreg_bridge.rs", "regkey_method", "RegKey"),
    b("service_bridge.rs", "service_method", "Service"),
    b("service_bridge.rs", "manager_method", "ServiceManager"),
    b("wmi_bridge.rs", "wmi_method", "WmiConnection"),
];

const fn b(file: &'static str, func: &'static str, recv: &'static str) -> Bridge {
    Bridge { file, func, recv }
}

/// Just grabs every string literal in a bridge function. Simpler than understanding each dispatch style.
/// A stray literal only makes the check too permissive, a missed one would reject working code.
struct LitCollector<'a> {
    names: BTreeSet<String>,
    variants: &'a Variants,
}

impl<'a> LitCollector<'a> {
    fn new(variants: &'a Variants) -> Self {
        LitCollector {
            names: BTreeSet::new(),
            variants,
        }
    }

    fn take_variant(&mut self, ident: &str) {
        if let Some(name) = self.variants.get(ident) {
            self.names.insert(name.clone());
        }
    }

    fn take(&mut self, value: String) {
        // method names only
        if !value.is_empty()
            && value
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            self.names.insert(value);
        }
    }

    /// Macro bodies are raw tokens, the ast visitor doesn't see `matches!(name, "lock")`.
    fn take_tokens(&mut self, tokens: proc_macro2::TokenStream) {
        use proc_macro2::TokenTree;
        let trees: Vec<TokenTree> = tokens.into_iter().collect();
        for (i, tree) in trees.iter().enumerate() {
            match tree {
                TokenTree::Literal(lit) => {
                    let text = lit.to_string();
                    if let Some(inner) = text.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
                        self.take(inner.to_string());
                    }
                }
                TokenTree::Group(group) => self.take_tokens(group.stream()),
                // `BuiltinId::Name` inside a macro body
                TokenTree::Ident(ident) if i >= 3 && is_id_prefix(&trees[i - 3]) => {
                    if let (TokenTree::Punct(a), TokenTree::Punct(b)) =
                        (&trees[i - 2], &trees[i - 1])
                        && a.as_char() == ':'
                        && b.as_char() == ':'
                    {
                        self.take_variant(&ident.to_string());
                    }
                }
                _ => {}
            }
        }
    }
}

fn is_id_prefix(tree: &proc_macro2::TokenTree) -> bool {
    matches!(tree, proc_macro2::TokenTree::Ident(ident) if ident == "BuiltinId")
}

impl<'ast> Visit<'ast> for LitCollector<'_> {
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        self.take_tokens(mac.tokens.clone());
        syn::visit::visit_macro(self, mac);
    }

    fn visit_lit_str(&mut self, lit: &'ast syn::LitStr) {
        self.take(lit.value());
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        if path.segments.len() == 2 && path.segments[0].ident == "BuiltinId" {
            self.take_variant(&path.segments[1].ident.to_string());
        }
        syn::visit::visit_path(self, path);
    }
}

/// Union over every match. A bridge split by `#[cfg]` has the function twice and the stub copy is empty.
struct FnFinder<'a> {
    want: &'a str,
    variants: &'a Variants,
    found: Option<BTreeSet<String>>,
}

impl<'ast> Visit<'ast> for FnFinder<'_> {
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if item.sig.ident == self.want {
            let mut c = LitCollector::new(self.variants);
            c.visit_block(&item.block);
            self.found.get_or_insert_with(BTreeSet::new).extend(c.names);
        }
        syn::visit::visit_item_fn(self, item);
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if item.sig.ident == self.want {
            let mut c = LitCollector::new(self.variants);
            c.visit_block(&item.block);
            self.found.get_or_insert_with(BTreeSet::new).extend(c.names);
        }
        syn::visit::visit_impl_item_fn(self, item);
    }
}

fn harvest(dir: &Path, file: &str, func: &str, variants: &Variants) -> Option<BTreeSet<String>> {
    let text = std::fs::read_to_string(dir.join(file)).ok()?;
    let ast = syn::parse_file(&text).ok()?;
    let mut finder = FnFinder {
        want: func,
        variants,
        found: None,
    };
    finder.visit_file(&ast);
    finder.found
}

/// The VM dispatches some ids itself, spread over many functions.
fn harvest_file(dir: &Path, file: &str, variants: &Variants) -> BTreeSet<String> {
    let text = std::fs::read_to_string(dir.join(file))
        .unwrap_or_else(|e| panic!("cannot read {file}: {e}"));
    let ast = syn::parse_file(&text).unwrap_or_else(|e| panic!("cannot parse {file}: {e}"));
    let mut c = LitCollector::new(variants);
    c.visit_file(&ast);
    c.names
}

pub fn generate(interpreter_dir: &Path, rows: &[MethodRow]) -> String {
    let variants: Variants = rows
        .iter()
        .map(|row| (camel(&row.name), row.name.clone()))
        .collect();
    let mut out = String::new();
    out.push_str(
        "// Generated by build.rs from the bridge sources. Do not edit.\n\
         // See src/bridge_tables_build.rs for how and why.\n\n",
    );

    let mut rows: Vec<String> = Vec::new();
    for bridge in BRIDGES {
        let names =
            harvest(interpreter_dir, bridge.file, bridge.func, &variants).unwrap_or_else(|| {
                panic!(
                    "bridge function `{}` not found in {}. It was renamed or moved, \
                 which would silently empty its coverage table.",
                    bridge.func, bridge.file
                )
            });
        let list: Vec<String> = names.iter().map(|n| format!("{n:?}")).collect();
        rows.push(format!(
            "    BridgeTable {{ recv: {:?}, names: &[{}] }},",
            bridge.recv,
            list.join(", ")
        ));
    }

    let _ = writeln!(
        out,
        "pub const BRIDGE_TABLES: &[BridgeTable] = &[\n{}\n];\n",
        rows.join("\n")
    );

    // the VM handles some methods itself in `vm_method.rs`
    let builtin = harvest_file(interpreter_dir, "vm_method.rs", &variants);
    let list: Vec<String> = builtin.iter().map(|n| format!("{n:?}")).collect();
    let _ = writeln!(
        out,
        "pub const BUILTIN_IDS: &[&str] = &[{}];\n",
        list.join(", ")
    );

    out
}
