//! Discover and parse every module file a script pulls in through `mod`
//! declarations, following the same directory rules as rustc. The result is a
//! flat list of modules, each with its path from the crate root, plus the file
//! set the checker mirrors into its cargo project.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use syn::Item;

/// One module of the script, file backed or inline.
pub struct ModuleSrc {
    /// Segments from the crate root, empty for the root module.
    pub path: Vec<String>,
    /// The module's items, with `mod` declarations already expanded away.
    pub items: Vec<Item>,
}

/// The whole script as parsed source files.
pub struct Program {
    /// Root module first, then discovery order.
    pub modules: Vec<ModuleSrc>,
    /// Every source file: path relative to the script directory, and content.
    /// The root script is first, stored as `main.rs`.
    pub files: Vec<(PathBuf, String)>,
}

pub fn load(script_path: &Path, root_source: &str) -> Result<Program> {
    let ast = syn::parse_file(root_source)
        .map_err(|e| anyhow!("parse error: {e}"))?;
    let dir = script_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut program = Program {
        modules: Vec::new(),
        files: vec![(PathBuf::from("main.rs"), root_source.to_string())],
    };
    let root = collect(&mut program, &dir, &dir, Vec::new(), ast.items)?;
    program.modules.insert(0, root);
    Ok(program)
}

/// Walk one module's items, loading `mod name;` files and expanding inline
/// `mod name { .. }` blocks. `children_dir` is where this module's child
/// files live. Returns this module with its `mod` items stripped; discovered
/// children are appended to `program.modules` depth first.
fn collect(
    program: &mut Program,
    script_dir: &Path,
    children_dir: &Path,
    path: Vec<String>,
    items: Vec<Item>,
) -> Result<ModuleSrc> {
    let mut kept = Vec::with_capacity(items.len());
    let mut seen: Vec<String> = Vec::new();
    for item in items {
        let Item::Mod(m) = item else {
            kept.push(item);
            continue;
        };
        let name = m.ident.to_string();
        if m.attrs.iter().any(|a| a.path().is_ident("path")) {
            bail!("unsupported feature: #[path] on `mod {name}`");
        }
        if seen.contains(&name) {
            bail!("module `{name}` is declared twice in {}", module_label(&path));
        }
        seen.push(name.clone());
        let mut child_path = path.clone();
        child_path.push(name.clone());
        let child_dir = children_dir.join(&name);
        let child_items = match m.content {
            Some((_, inline_items)) => inline_items,
            None => load_file(program, script_dir, children_dir, &name, &child_path)?,
        };
        let child = collect(program, script_dir, &child_dir, child_path, child_items)?;
        program.modules.push(child);
    }
    Ok(ModuleSrc { path, items: kept })
}

/// Read and parse the file behind `mod name;`, trying `name.rs` then
/// `name/mod.rs` inside the declaring module's directory.
fn load_file(
    program: &mut Program,
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
    let ast = syn::parse_file(&source)
        .map_err(|e| anyhow!("parse error in {}: {e}", file.display()))?;
    let rel = file.strip_prefix(script_dir).unwrap_or(&file).to_path_buf();
    program.files.push((rel, source));
    Ok(ast.items)
}

fn module_label(path: &[String]) -> String {
    if path.is_empty() {
        "the script root".to_string()
    } else {
        format!("module `{}`", path.join("::"))
    }
}
