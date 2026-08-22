//! The std method surface harvested from `rust-src` into `std_surface.txt`, against the catalog rows
//! and the interpreter's bridged surface. The catalog is hand written, so this makes its gaps
//! measurable.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::lang::catalog::{METHODS, RecvClass};

pub const SURFACE_FILE: &str = "crates/differential/std_surface.txt";

/// Receiver groups by the std source files that hold their inherent methods.
const SOURCES: &[(&str, &[&str])] = &[
    (
        "Int",
        &["core/src/num/int_macros.rs", "core/src/num/uint_macros.rs"],
    ),
    (
        "Float",
        &[
            "core/src/num/f32.rs",
            "core/src/num/f64.rs",
            "std/src/num/f32.rs",
            "std/src/num/f64.rs",
        ],
    ),
    ("Bool", &["core/src/bool.rs"]),
    (
        "Str",
        &[
            "core/src/str/mod.rs",
            "alloc/src/string.rs",
            "alloc/src/str.rs",
        ],
    ),
    ("Vec", &["alloc/src/vec/mod.rs", "core/src/slice/mod.rs"]),
    ("Opt", &["core/src/option.rs"]),
    ("Res", &["core/src/result.rs"]),
    ("Map", &["std/src/collections/hash/map.rs"]),
    ("Set", &["std/src/collections/hash/set.rs"]),
    ("Char", &["core/src/char/methods.rs"]),
    ("Iter", &["core/src/iter/traits/iterator.rs"]),
];

/// Std trait methods the harvested files don't declare.
pub const TRAIT_METHODS: &[&str] = &[
    "clone",
    "cloned",
    "copied",
    "to_vec",
    "to_owned",
    "to_string",
    "into",
    "into_iter",
    "iter",
    "collect",
    "map",
    "filter",
    "sum",
    "product",
    "len",
    "insert",
    "sort",
    "dedup",
    "max",
    "min",
    "count",
    "flatten",
    "rev",
    "skip",
    "take",
    "step_by",
    "zip",
    "enumerate",
    "ok",
    "unwrap_or",
    "map_err",
    "unwrap_err",
    "union",
    "intersection",
    "difference",
    "binary_search",
    "remove",
    "into_keys",
    "into_values",
    "as_str",
    "as_slice",
    "split_first",
    "split_last",
    "windows",
    "chunks",
    "concat",
    "join",
    "repeat",
    "bytes",
    "chars",
    "nth",
    "parse",
    "lines",
    "split",
    "split_whitespace",
    "matches",
    "get",
    "first",
    "last",
    "contains",
    "contains_key",
    "is_empty",
    "from",
];

pub type Surface = BTreeMap<String, BTreeSet<String>>;

pub fn load(root: &Path) -> Result<Surface> {
    let path = root.join(SURFACE_FILE);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("read {}, run `surface --refresh` first", path.display()))?;
    Ok(parse(&text))
}

pub fn parse(text: &str) -> Surface {
    let mut out: Surface = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((recv, name)) = line.split_once(' ') {
            out.entry(recv.to_string())
                .or_default()
                .insert(name.trim().to_string());
        }
    }
    out
}

pub fn refresh(root: &Path) -> Result<String> {
    let library = rust_src_library()?;
    let mut lines = vec![
        "# Stable methods taking self on the generator's receiver types, read".to_string(),
        "# from the toolchain's rust-src by `rustscript-differential surface".to_string(),
        "# --refresh`. One `Receiver name` per line.".to_string(),
    ];
    for (recv, files) in SOURCES {
        let mut names = BTreeSet::new();
        for file in *files {
            let path = library.join(file);
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?;
            names.extend(stable_self_methods(&text));
        }
        for name in names {
            lines.push(format!("{recv} {name}"));
        }
    }
    let text = lines.join("\n") + "\n";
    std::fs::write(root.join(SURFACE_FILE), &text)?;
    Ok(text)
}

fn rust_src_library() -> Result<PathBuf> {
    let output = Command::new("rustc")
        .arg("--print")
        .arg("sysroot")
        .output()?;
    let sysroot = String::from_utf8(output.stdout)?.trim().to_string();
    let library = Path::new(&sysroot).join("lib/rustlib/src/rust/library");
    if !library.is_dir() {
        bail!(
            "rust-src is not installed, run `rustup component add rust-src` ({} is missing)",
            library.display()
        );
    }
    Ok(library)
}

/// Stable, documented, safe `fn`s taking `self` in 1 source file, inherent impls only. Trait impl
/// blocks are skipped by brace depth so `fmt` and `eq` don't count.
fn stable_self_methods(text: &str) -> BTreeSet<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = BTreeSet::new();
    let mut depth = 0i32;
    let mut trait_impl_until: Option<i32> = None;
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trait_impl_until.is_none()
            && (trimmed.starts_with("impl") || trimmed.starts_with("unsafe impl"))
            && trimmed.contains(" for ")
        {
            trait_impl_until = Some(depth);
        }
        let inside_trait_impl = trait_impl_until.is_some();
        let opens = i32::try_from(line.matches('{').count()).unwrap_or(0);
        let closes = i32::try_from(line.matches('}').count()).unwrap_or(0);
        depth += opens - closes;
        if let Some(base) = trait_impl_until
            && depth <= base
            && closes > 0
        {
            trait_impl_until = None;
        }
        if inside_trait_impl {
            continue;
        }
        let Some(name) = fn_name(line) else {
            continue;
        };
        if name.starts_with('_') || name.starts_with("spec_") || trimmed.contains("unsafe fn ") {
            continue;
        }
        // the receiver may sit on the next line
        let head: String = lines[index..(index + 4).min(lines.len())].join(" ");
        let after_paren = head.split_once('(').map_or("", |(_, rest)| rest);
        if !after_paren.trim_start().starts_with("self")
            && !after_paren.trim_start().starts_with("&self")
            && !after_paren.trim_start().starts_with("&mut self")
            && !after_paren.trim_start().starts_with("mut self")
            && !after_paren.trim_start().starts_with("&'a self")
            && !after_paren.trim_start().starts_with("&'a mut self")
        {
            continue;
        }
        if attributes_above(&lines, index).iter().any(|attr| {
            attr.contains("#[unstable")
                || attr.contains("doc(hidden)")
                || attr.contains("#[deprecated")
        }) {
            continue;
        }
        out.insert(name);
    }
    out
}

fn fn_name(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix("pub const unsafe fn ")
        .or_else(|| trimmed.strip_prefix("pub const fn "))
        .or_else(|| trimmed.strip_prefix("pub unsafe fn "))
        .or_else(|| trimmed.strip_prefix("pub fn "))
        .or_else(|| trimmed.strip_prefix("fn "))?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

fn attributes_above<'a>(lines: &[&'a str], index: usize) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut at = index;
    while at > 0 {
        at -= 1;
        let line = lines[at].trim_start();
        if line.starts_with("#[")
            || line.starts_with("///")
            || line.starts_with("//")
            || line.is_empty()
        {
            out.push(line);
        } else {
            break;
        }
    }
    out
}

fn catalog_by_group() -> BTreeMap<&'static str, BTreeSet<String>> {
    let mut out: BTreeMap<&'static str, BTreeSet<String>> = BTreeMap::new();
    for method in METHODS.iter() {
        let group = match method.recv {
            RecvClass::Int | RecvClass::SignedInt | RecvClass::UnsignedInt => "Int",
            RecvClass::Float => "Float",
            RecvClass::Bool => "Bool",
            RecvClass::Char => "Char",
            RecvClass::Str => "Str",
            RecvClass::Vec | RecvClass::VecOfVec => "Vec",
            RecvClass::Opt => "Opt",
            RecvClass::Res => "Res",
            RecvClass::Map => "Map",
            RecvClass::Set => "Set",
        };
        out.entry(group)
            .or_default()
            .extend(template_methods(method.template));
    }
    out
}

pub fn template_methods(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(dot) = rest.find('.') {
        rest = &rest[dot + 1..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let after = &rest[name.len()..];
        if !name.is_empty() && (after.starts_with('(') || after.starts_with("::<")) {
            out.push(name);
        }
    }
    out
}

/// The `rust supported` listing per receiver, universal rows copied into every receiver.
pub fn interpreter_surface(listing: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut universal = BTreeSet::new();
    let mut current: Option<String> = None;
    for line in listing.lines() {
        if let Some(label) = line.strip_suffix(':') {
            current = Some(label.to_string());
            continue;
        }
        let names: Vec<String> = line
            .split(',')
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect();
        let Some(label) = &current else { continue };
        if label.starts_with("any value") || label.starts_with("builtin") {
            universal.extend(names);
        } else {
            out.entry(label.clone()).or_default().extend(names);
        }
    }
    for names in out.values_mut() {
        names.extend(universal.iter().cloned());
    }
    // number methods have no table of their own, so the universal rows are the whole number surface
    out.insert("number".to_string(), universal);
    out
}

/// Universal rows kept apart under `any value`, so the count doesn't multiply them per receiver.
pub fn interpreter_surface_raw(listing: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in listing.lines() {
        if let Some(label) = line.strip_suffix(':') {
            current = Some(label.to_string());
            continue;
        }
        let Some(label) = &current else { continue };
        let key = if label.starts_with("any value") || label.starts_with("builtin") {
            "any value".to_string()
        } else {
            label.clone()
        };
        out.entry(key).or_default().extend(
            line.split(',')
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty()),
        );
    }
    out
}

fn interpreter_label(group: &str) -> &'static str {
    match group {
        "Int" | "Float" | "Bool" => "number",
        "Str" => "String and str",
        "Vec" => "Vec",
        "Opt" => "Option",
        "Res" => "Result",
        "Map" | "Set" => "Map",
        "Char" => "Char",
        _ => "Iterator",
    }
}

pub struct Report {
    /// std names the catalog never generates
    pub uncovered_by_catalog: BTreeMap<String, Vec<String>>,
    /// std names the interpreter doesn't implement
    pub missing_in_interpreter: BTreeMap<String, Vec<String>>,
    /// template calls that are not std on any receiver
    pub catalog_not_std: Vec<String>,
    pub std_total: usize,
}

impl Report {
    pub fn render(&self) -> String {
        let mut out = String::new();
        let covered = self.std_total
            - self
                .uncovered_by_catalog
                .values()
                .map(Vec::len)
                .sum::<usize>();
        out.push_str(&format!(
            "std surface: {} methods on the generator's receiver types, {covered} generated by the catalog\n",
            self.std_total
        ));
        for (group, names) in &self.missing_in_interpreter {
            out.push_str(&format!(
                "  {group}: {} std methods the interpreter does not implement: {}\n",
                names.len(),
                names.join(", ")
            ));
        }
        for (group, names) in &self.uncovered_by_catalog {
            out.push_str(&format!(
                "  {group}: {} std methods no catalog row generates: {}\n",
                names.len(),
                names.join(", ")
            ));
        }
        if !self.catalog_not_std.is_empty() {
            out.push_str(&format!(
                "  catalog calls that are not std: {}\n",
                self.catalog_not_std.join(", ")
            ));
        }
        out
    }
}

pub fn report(surface: &Surface, listing: &str) -> Report {
    let catalog = catalog_by_group();
    let interpreter = interpreter_surface(listing);
    let mut uncovered_by_catalog = BTreeMap::new();
    let mut missing_in_interpreter = BTreeMap::new();
    let mut std_total = 0;
    for (group, names) in surface {
        std_total += names.len();
        let generated = catalog.get(group.as_str()).cloned().unwrap_or_default();
        let implemented = interpreter
            .get(interpreter_label(group))
            .cloned()
            .unwrap_or_default();
        let uncovered: Vec<String> = names
            .iter()
            .filter(|name| !generated.contains(*name))
            .cloned()
            .collect();
        let missing: Vec<String> = names
            .iter()
            .filter(|name| !implemented.contains(*name))
            .cloned()
            .collect();
        if !uncovered.is_empty() {
            uncovered_by_catalog.insert(group.clone(), uncovered);
        }
        if !missing.is_empty() {
            missing_in_interpreter.insert(group.clone(), missing);
        }
    }
    let all_std: BTreeSet<&String> = surface.values().flatten().collect();
    let mut catalog_not_std: Vec<String> = catalog
        .values()
        .flatten()
        .filter(|name| !all_std.contains(name) && !TRAIT_METHODS.contains(&name.as_str()))
        .cloned()
        .collect();
    catalog_not_std.sort();
    catalog_not_std.dedup();
    Report {
        uncovered_by_catalog,
        missing_in_interpreter,
        catalog_not_std,
        std_total,
    }
}
