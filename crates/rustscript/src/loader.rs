//! Loads every module a script pulls in through `mod` with the `rustc`
//! directory rules. Local `path` crates are grafted in as top level modules,
//! the checker sees them as real path dependencies.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use syn::{Item, LitStr};

pub struct ModuleSrc {
    pub path: Vec<String>,
    /// `mod` declarations are already expanded away.
    pub items: Vec<Item>,
    /// Inline modules carry their parent's file. Shown in error traces.
    pub file: Arc<str>,
    /// `crate::` pins here and `super` must not walk past it.
    pub crate_root: bool,
}

pub struct CrateDep {
    /// Also the top level module it grafts as.
    pub name: String,
    pub dir: PathBuf,
    /// Kept only so a change re-triggers the check.
    pub files: Vec<(PathBuf, String)>,
}

pub struct Program {
    /// Root first, then discovery order, then grafted crates.
    pub modules: Vec<ModuleSrc>,
    /// The root script is first under its own name so diagnostics show it.
    pub files: Vec<(PathBuf, String)>,
    pub crate_deps: Vec<CrateDep>,
    /// `fn main` carries `#[tokio::main]`.
    pub tokio_main: bool,
}

pub fn load(script_path: &Path, root_source: &str) -> Result<Program> {
    let ast = syn::parse_file(root_source).map_err(|e| anyhow!("parse error: {e}"))?;
    let dir = script_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut modules: Vec<ModuleSrc> = Vec::new();
    let root_file = root_file_name(script_path);
    let mut files: Vec<(PathBuf, String)> =
        vec![(PathBuf::from(&root_file), root_source.to_string())];
    let mut root = collect(
        &mut modules,
        &mut files,
        &dir,
        &dir,
        Vec::new(),
        Arc::from(root_file.as_str()),
        ast.items,
    )?;
    root.crate_root = true;
    modules.insert(0, root);
    let tokio_main = detect_tokio_main(&modules[0].items)?;
    let crate_deps = graft_crate_deps(&mut modules, &files, script_path)?;
    Ok(Program {
        modules,
        files,
        crate_deps,
        tokio_main,
    })
}

/// An extensionless name falls back to `main.rs` so cargo builds it.
fn root_file_name(script_path: &Path) -> String {
    match script_path.file_name().and_then(|n| n.to_str()) {
        Some(name) if Path::new(name).extension() == Some(OsStr::new("rs")) => name.to_string(),
        _ => "main.rs".to_string(),
    }
}

/// Only the multi thread runtime exists, so any explicit flavor is rejected.
fn detect_tokio_main(items: &[Item]) -> Result<bool> {
    for item in items {
        let Item::Fn(f) = item else { continue };
        if f.sig.ident != "main" {
            continue;
        }
        for attr in &f.attrs {
            let segs: Vec<String> = attr
                .path()
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            if segs.last().map(String::as_str) != Some("main") || !segs.iter().any(|s| s == "tokio")
            {
                continue;
            }
            if matches!(attr.meta, syn::Meta::List(_)) {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("flavor") {
                        let flavor: LitStr = meta.value()?.parse()?;
                        if flavor.value() != "multi_thread" {
                            return Err(meta.error(
                                "only #[tokio::main] with the multi_thread flavor is supported",
                            ));
                        }
                    }
                    Ok(())
                })?;
            }
            return Ok(true);
        }
    }
    Ok(false)
}

/// Matched narrowly so `#[cfg(not(test))]` is still kept.
fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("cfg")
            && matches!(&a.meta, syn::Meta::List(list) if list.tokens.to_string().replace(' ', "") == "test")
    })
}

fn item_attrs(item: &Item) -> &[syn::Attribute] {
    match item {
        Item::Const(i) => &i.attrs,
        Item::Enum(i) => &i.attrs,
        Item::Fn(i) => &i.attrs,
        Item::Impl(i) => &i.attrs,
        Item::Mod(i) => &i.attrs,
        Item::Static(i) => &i.attrs,
        Item::Struct(i) => &i.attrs,
        Item::Trait(i) => &i.attrs,
        Item::Type(i) => &i.attrs,
        Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

/// Real trees are a handful of levels, so this only ever catches a loop.
const MAX_MODULE_DEPTH: usize = 64;

/// Returns this module with its `mod` items stripped. Children are appended
/// depth first.
fn collect(
    modules: &mut Vec<ModuleSrc>,
    files: &mut Vec<(PathBuf, String)>,
    script_dir: &Path,
    children_dir: &Path,
    path: Vec<String>,
    file: Arc<str>,
    items: Vec<Item>,
) -> Result<ModuleSrc> {
    // A `#[path]` pointing at its own file once overflowed the native stack
    // with a bare "fatal runtime error".
    if path.len() > MAX_MODULE_DEPTH {
        // Only the tail, the full path is one segment repeated 60 times.
        bail!(
            "module `{}` nests deeper than {MAX_MODULE_DEPTH} levels, which usually means a `#[path]` points back at its own file",
            path.last().map_or("", String::as_str)
        );
    }
    let mut kept = Vec::with_capacity(items.len());
    let mut seen: Vec<String> = Vec::new();
    for item in items {
        // A `#[cfg(test)]` item never runs here, so skip its test only
        // constructs.
        if is_cfg_test(item_attrs(&item)) {
            continue;
        }
        let Item::Mod(m) = item else {
            kept.push(item);
            continue;
        };
        let name = m.ident.to_string();
        if seen.contains(&name) {
            bail!(
                "module `{name}` is declared twice in {}",
                module_label(&path)
            );
        }
        seen.push(name.clone());
        let mut child_path = path.clone();
        child_path.push(name.clone());
        // A `#[path]` file's own submodules resolve relative to that file's
        // directory, like Rust does. This lets a bin keep its modules in a
        // subdirectory so cargo does not treat each as a binary.
        let path_attr = mod_path_attr(&m);
        let child_dir;
        let (child_items, child_file) = match m.content {
            Some((_, inline_items)) => {
                child_dir = children_dir.join(&name);
                (inline_items, file.clone())
            }
            None => {
                if let Some(rel) = &path_attr {
                    let target = children_dir.join(rel);
                    let loaded = load_file_at(files, script_dir, &target, &child_path)?;
                    child_dir = target
                        .parent()
                        .map_or_else(|| children_dir.to_path_buf(), Path::to_path_buf);
                    loaded
                } else {
                    child_dir = children_dir.join(&name);
                    load_file(files, script_dir, children_dir, &name, &child_path)?
                }
            }
        };
        let child = collect(
            modules,
            files,
            script_dir,
            &child_dir,
            child_path,
            child_file,
            child_items,
        )?;
        modules.push(child);
    }
    Ok(ModuleSrc {
        path,
        items: kept,
        file,
        crate_root: false,
    })
}

fn mod_path_attr(m: &syn::ItemMod) -> Option<String> {
    for attr in &m.attrs {
        if attr.path().is_ident("path")
            && let syn::Meta::NameValue(nv) = &attr.meta
            && let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
        {
            return Some(s.value());
        }
    }
    None
}

/// `name.rs` then `name/mod.rs`.
fn load_file(
    files: &mut Vec<(PathBuf, String)>,
    script_dir: &Path,
    children_dir: &Path,
    name: &str,
    child_path: &[String],
) -> Result<(Vec<Item>, Arc<str>)> {
    let flat = children_dir.join(format!("{name}.rs"));
    let nested = children_dir.join(name).join("mod.rs");
    let file = match (flat.is_file(), nested.is_file()) {
        (true, true) => bail!(
            "module `{}` has both {} and {}",
            child_path.join("::"),
            flat.display(),
            nested.display()
        ),
        (true, false) => flat,
        (false, true) => nested,
        (false, false) => bail!(
            "cannot find module `{}`: neither {} nor {} exists",
            child_path.join("::"),
            flat.display(),
            nested.display()
        ),
    };
    load_file_at(files, script_dir, &file, child_path)
}

fn load_file_at(
    files: &mut Vec<(PathBuf, String)>,
    script_dir: &Path,
    file: &Path,
    child_path: &[String],
) -> Result<(Vec<Item>, Arc<str>)> {
    if !file.is_file() {
        bail!(
            "cannot find module `{}`: {} does not exist",
            child_path.join("::"),
            file.display()
        );
    }
    let source = std::fs::read_to_string(file)
        .map_err(|e| anyhow!("cannot read {}: {e}", file.display()))?;
    let ast =
        syn::parse_file(&source).map_err(|e| anyhow!("parse error in {}: {e}", file.display()))?;
    let rel = file.strip_prefix(script_dir).unwrap_or(file).to_path_buf();
    let display: Arc<str> = Arc::from(rel.to_string_lossy().as_ref());
    files.push((rel, source));
    Ok((ast.items, display))
}

/// Whether the script's own sources name this crate. Grafting an unused one
/// pulls its whole surface into `rust check`, which once rejected a script for
/// methods only a helper library calls.
fn uses_crate(files: &[(PathBuf, String)], module_name: &str) -> bool {
    let needle = format!("{module_name}::");
    files.iter().any(|(_, source)| source.contains(&needle))
}

fn graft_crate_deps(
    modules: &mut Vec<ModuleSrc>,
    files: &[(PathBuf, String)],
    script_path: &Path,
) -> Result<Vec<CrateDep>> {
    let mut deps = Vec::new();
    for (name, dir) in local_path_deps(script_path) {
        let src_dir = dir.join("src");
        let lib = src_dir.join("lib.rs");
        if !lib.is_file() {
            continue;
        }
        // `verify-common` is `verify_common` in `use`, the grafted module must
        // match.
        let module_name = name.replace('-', "_");
        if !uses_crate(files, &module_name) {
            continue;
        }
        let source = std::fs::read_to_string(&lib)
            .map_err(|e| anyhow!("cannot read {}: {e}", lib.display()))?;
        let ast = syn::parse_file(&source)
            .map_err(|e| anyhow!("parse error in {}: {e}", lib.display()))?;
        let mut crate_files: Vec<(PathBuf, String)> = vec![(PathBuf::from("lib.rs"), source)];
        let mut root = collect(
            modules,
            &mut crate_files,
            &src_dir,
            &src_dir,
            vec![module_name],
            Arc::from("lib.rs"),
            ast.items,
        )?;
        root.crate_root = true;
        modules.push(root);
        deps.push(CrateDep {
            name,
            dir,
            files: crate_files,
        });
    }
    Ok(deps)
}

/// The local `path` entries of the nearest `Cargo.toml`, as absolute dirs.
fn local_path_deps(script_path: &Path) -> Vec<(String, PathBuf)> {
    let Some(manifest) = nearest_manifest(script_path) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return Vec::new();
    };
    let Ok(value) = toml::from_str::<toml::Value>(&text) else {
        return Vec::new();
    };
    let manifest_dir = manifest.parent().unwrap_or(Path::new("."));
    let Some(deps) = value.get("dependencies").and_then(|d| d.as_table()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, spec) in deps {
        if let Some(rel) = spec
            .as_table()
            .and_then(|t| t.get("path"))
            .and_then(|p| p.as_str())
        {
            // The checker writes this into a manifest under the cache dir, so
            // a relative path would resolve against the wrong root.
            let dir = manifest_dir.join(rel);
            let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
            out.push((name.clone(), dir));
        }
    }
    out
}

/// Canonicalized first, so `rust kimai.rs` still walks up the real tree.
fn nearest_manifest(script_path: &Path) -> Option<PathBuf> {
    let absolute = std::fs::canonicalize(script_path).unwrap_or_else(|_| script_path.to_path_buf());
    let mut dir = absolute.parent();
    while let Some(d) = dir {
        let candidate = d.join("Cargo.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = d.parent();
    }
    None
}

fn module_label(path: &[String]) -> String {
    if path.is_empty() {
        "the script root".to_string()
    } else {
        format!("module `{}`", path.join("::"))
    }
}
