//! Discover and parse every module file a script pulls in through `mod`
//! declarations, following the same directory rules as rustc. The result is a
//! flat list of modules, each with its path from the crate root, plus the file
//! set the checker mirrors into its cargo project.
//!
//! A script that lives inside a cargo crate may also depend on a local `path`
//! crate, for example a `shared` helper library. Such a crate is grafted in as
//! a top level module so `use shared::x` resolves at runtime without a `mod`
//! declaration, while the checker sees it as a real path dependency.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use syn::{Item, LitStr};

/// One module of the script, file backed or inline.
pub struct ModuleSrc {
    /// Segments from the crate root, empty for the root module.
    pub path: Vec<String>,
    /// The module's items, with `mod` declarations already expanded away.
    pub items: Vec<Item>,
}

/// A local `path` dependency crate that the script uses, grafted in from
/// source. The checker adds it to the cargo project as a path dependency.
pub struct CrateDep {
    /// The crate name, which is also the top level module it grafts as.
    pub name: String,
    /// The crate directory, the one that holds its `Cargo.toml`.
    pub dir: PathBuf,
    /// The crate's source files, kept only so a change re-triggers the check.
    pub files: Vec<(PathBuf, String)>,
}

/// The whole script as parsed source files.
pub struct Program {
    /// Root module first, then discovery order, then grafted crate modules.
    pub modules: Vec<ModuleSrc>,
    /// Every source file: path relative to the script directory, and content.
    /// The root script is first, stored as `main.rs`.
    pub files: Vec<(PathBuf, String)>,
    /// Local crates the script pulls in through a `path` dependency.
    pub crate_deps: Vec<CrateDep>,
    /// True when `fn main` carries `#[tokio::main]`, routing the script to the
    /// parallel engine instead of the single threaded fast engine.
    pub tokio_main: bool,
}

pub fn load(script_path: &Path, root_source: &str) -> Result<Program> {
    let ast = syn::parse_file(root_source).map_err(|e| anyhow!("parse error: {e}"))?;
    let dir = script_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut modules: Vec<ModuleSrc> = Vec::new();
    let mut files: Vec<(PathBuf, String)> =
        vec![(PathBuf::from("main.rs"), root_source.to_string())];
    let root = collect(&mut modules, &mut files, &dir, &dir, Vec::new(), ast.items)?;
    modules.insert(0, root);
    let tokio_main = detect_tokio_main(&modules[0].items)?;
    let crate_deps = graft_crate_deps(&mut modules, script_path)?;
    Ok(Program {
        modules,
        files,
        crate_deps,
        tokio_main,
    })
}

/// Look for `#[tokio::main]` on `fn main`. Only the multi thread runtime is
/// offered, so a `current_thread` flavor is rejected with a clear error, as is
/// any other explicit flavor. A missing flavor means the multi thread default.
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

/// Whether an item is gated to `#[cfg(test)]`, matched narrowly so a
/// `#[cfg(not(test))]` item is still kept.
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

/// Walk one module's items, loading `mod name;` files and expanding inline
/// `mod name { .. }` blocks. `children_dir` is where this module's child
/// files live. Returns this module with its `mod` items stripped; discovered
/// children are appended to `modules` depth first, their files to `files`.
fn collect(
    modules: &mut Vec<ModuleSrc>,
    files: &mut Vec<(PathBuf, String)>,
    script_dir: &Path,
    children_dir: &Path,
    path: Vec<String>,
    items: Vec<Item>,
) -> Result<ModuleSrc> {
    let mut kept = Vec::with_capacity(items.len());
    let mut seen: Vec<String> = Vec::new();
    for item in items {
        // A `#[cfg(test)]` item, usually a `mod tests`, never runs under the
        // interpreter, so skip it rather than compile its test-only constructs.
        if is_cfg_test(item_attrs(&item)) {
            continue;
        }
        let Item::Mod(m) = item else {
            kept.push(item);
            continue;
        };
        let name = m.ident.to_string();
        if m.attrs.iter().any(|a| a.path().is_ident("path")) {
            bail!("unsupported feature: #[path] on `mod {name}`");
        }
        if seen.contains(&name) {
            bail!(
                "module `{name}` is declared twice in {}",
                module_label(&path)
            );
        }
        seen.push(name.clone());
        let mut child_path = path.clone();
        child_path.push(name.clone());
        let child_dir = children_dir.join(&name);
        let child_items = match m.content {
            Some((_, inline_items)) => inline_items,
            None => load_file(files, script_dir, children_dir, &name, &child_path)?,
        };
        let child = collect(
            modules,
            files,
            script_dir,
            &child_dir,
            child_path,
            child_items,
        )?;
        modules.push(child);
    }
    Ok(ModuleSrc { path, items: kept })
}

/// Read and parse the file behind `mod name;`, trying `name.rs` then
/// `name/mod.rs` inside the declaring module's directory.
fn load_file(
    files: &mut Vec<(PathBuf, String)>,
    script_dir: &Path,
    children_dir: &Path,
    name: &str,
    child_path: &[String],
) -> Result<Vec<Item>> {
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
    let source = std::fs::read_to_string(&file)
        .map_err(|e| anyhow!("cannot read {}: {e}", file.display()))?;
    let ast =
        syn::parse_file(&source).map_err(|e| anyhow!("parse error in {}: {e}", file.display()))?;
    let rel = file.strip_prefix(script_dir).unwrap_or(&file).to_path_buf();
    files.push((rel, source));
    Ok(ast.items)
}

/// Graft each local `path` dependency crate in as a top level module named
/// after the crate, loading its `src/lib.rs` and the module tree below it. The
/// runtime then resolves `use crate_name::..` against the grafted modules, and
/// the returned deps tell the checker to add them as path dependencies.
fn graft_crate_deps(modules: &mut Vec<ModuleSrc>, script_path: &Path) -> Result<Vec<CrateDep>> {
    let mut deps = Vec::new();
    for (name, dir) in local_path_deps(script_path) {
        let src_dir = dir.join("src");
        let lib = src_dir.join("lib.rs");
        if !lib.is_file() {
            continue;
        }
        let source = std::fs::read_to_string(&lib)
            .map_err(|e| anyhow!("cannot read {}: {e}", lib.display()))?;
        let ast = syn::parse_file(&source)
            .map_err(|e| anyhow!("parse error in {}: {e}", lib.display()))?;
        let mut files: Vec<(PathBuf, String)> = vec![(PathBuf::from("lib.rs"), source)];
        let root = collect(
            modules,
            &mut files,
            &src_dir,
            &src_dir,
            vec![name.clone()],
            ast.items,
        )?;
        modules.push(root);
        deps.push(CrateDep { name, dir, files });
    }
    Ok(deps)
}

/// Read the nearest `Cargo.toml` above the script and return its `[dependencies]`
/// entries that point at a local `path`, resolved to absolute directories.
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
            // The checker writes this dir into a throwaway manifest under the
            // cache dir, so a relative path would resolve against the wrong
            // root. Canonicalize to an absolute path pinned to the real crate.
            let dir = manifest_dir.join(rel);
            let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
            out.push((name.clone(), dir));
        }
    }
    out
}

/// The closest `Cargo.toml` at or above the script's directory, if any. The
/// path is canonicalized first, so a script run by a bare relative name like
/// `rust kimai.rs` still walks up the real tree to find the manifest that
/// grafts its `shared` crate.
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
