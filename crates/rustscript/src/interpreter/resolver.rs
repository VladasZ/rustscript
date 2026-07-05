//! Module aware name resolution. Every top level item gets a canonical key,
//! `foo::bar` for an item in module `foo`, a bare `bar` for a root item, so
//! single file scripts keep their old keys. Paths are resolved against the
//! module they appear in, at compile time for calls and at runtime for type
//! coercions, and anything that never lands on a user item falls through to
//! the bridge dispatch unchanged.

use std::collections::HashMap;
use std::rc::Rc;

use anyhow::{Result, bail};

/// Symbols of one module.
#[derive(Default)]
pub(super) struct ModuleSyms {
    pub path: Vec<String>,
    pub parent: Option<usize>,
    pub children: HashMap<String, usize>,
    /// Local name to global function index.
    pub fns: HashMap<String, u32>,
    /// Local name to global constant index.
    pub consts: HashMap<String, u32>,
    /// Local name to canonical struct key.
    pub structs: HashMap<String, Rc<str>>,
    /// Local name to canonical enum key.
    pub enums: HashMap<String, Rc<str>>,
    /// Local alias name to its target type.
    pub aliases: HashMap<String, Rc<syn::Type>>,
    /// Import name to the path it stands for.
    pub uses: HashMap<String, Vec<String>>,
    /// Prefixes of `use ...::*` imports, checked against user modules at load.
    pub globs: Vec<Vec<String>>,
}

pub(super) struct StructDef {
    pub ast: Rc<syn::ItemStruct>,
    pub module: usize,
}

/// What a path resolved to.
pub(super) enum Res {
    Fn(u32),
    Const(u32),
    Struct(Rc<str>),
    Enum(Rc<str>),
    /// `Type::rest` where the type is a user struct or enum: an associated
    /// function, a method used UFCS style, or an enum variant.
    TypeMember(Rc<str>, Vec<String>),
    /// A type alias hit exactly, resolved in its defining module.
    Alias(usize, Rc<syn::Type>),
    Module,
    /// Not a user item. Segments have imports already expanded.
    External(Vec<String>),
}

pub(super) struct Resolver {
    pub modules: Vec<ModuleSyms>,
    pub structs: HashMap<Rc<str>, StructDef>,
    pub enums: HashMap<Rc<str>, Rc<syn::ItemEnum>>,
}

/// Bound on import chains, so `pub use` cycles error instead of hanging.
const MAX_DEPTH: usize = 64;

impl Resolver {
    /// Canonical key for an item named `name` in module `m`.
    pub fn canon(&self, m: usize, name: &str) -> String {
        let path = &self.modules[m].path;
        if path.is_empty() {
            name.to_string()
        } else {
            format!("{}::{name}", path.join("::"))
        }
    }

    /// Resolve an expression path as written in module `m`.
    pub fn resolve(&self, m: usize, segs: &[String]) -> Result<Res> {
        self.resolve_at(m, segs, 0)
    }

    /// Resolve a `use` target written in module `m`. A `self` or `super` start
    /// pins the current module. Otherwise the first segment can still name one
    /// of the current module's own children, like `use ctx::Ctx` written beside
    /// `mod ctx`, which rustc resolves locally, so a submodule tries itself
    /// first and falls back to the crate root and the external prelude.
    fn resolve_use(&self, m: usize, segs: &[String], depth: usize) -> Result<Res> {
        if let Some("self" | "super") = segs.first().map(String::as_str) {
            return self.resolve_at(m, segs, depth);
        }
        // Only submodules need the local-first try. At the crate root the two
        // resolutions are the same walk, so the retry would just repeat the
        // whole use-alias expansion and blow up.
        if m != 0
            && let Ok(res) = self.resolve_at(m, segs, depth)
            && !matches!(res, Res::External(_))
        {
            return Ok(res);
        }
        self.resolve_at(0, segs, depth)
    }

    fn resolve_at(&self, mut m: usize, segs: &[String], depth: usize) -> Result<Res> {
        if depth > MAX_DEPTH {
            bail!("import chain too deep resolving `{}`", segs.join("::"));
        }
        let mut i = 0;
        // A leading crate/self/super run pins the starting module. After it,
        // only user items can match, so external fallback is off.
        let mut anchored = false;
        while i < segs.len() {
            match segs[i].as_str() {
                "crate" => m = 0,
                "self" => {}
                "super" => {
                    m = match self.modules[m].parent {
                        Some(p) => p,
                        None => bail!("`super` used at the crate root"),
                    };
                }
                _ => break,
            }
            anchored = true;
            i += 1;
        }
        if i == segs.len() {
            return Ok(Res::Module);
        }

        // Walk the remaining segments through the module tree.
        let start = m;
        loop {
            let seg = &segs[i];
            let last = i == segs.len() - 1;
            let syms = &self.modules[m];
            if let Some(&f) = syms.fns.get(seg) {
                if last {
                    return Ok(Res::Fn(f));
                }
                bail!("`{}` is a function, not a module", segs[..=i].join("::"));
            }
            if let Some(&c) = syms.consts.get(seg) {
                if last {
                    return Ok(Res::Const(c));
                }
                bail!("`{}` is a constant, not a module", segs[..=i].join("::"));
            }
            if let Some(canon) = syms.structs.get(seg) {
                return Ok(if last {
                    Res::Struct(canon.clone())
                } else {
                    Res::TypeMember(canon.clone(), segs[i + 1..].to_vec())
                });
            }
            if let Some(canon) = syms.enums.get(seg) {
                return Ok(if last {
                    Res::Enum(canon.clone())
                } else {
                    Res::TypeMember(canon.clone(), segs[i + 1..].to_vec())
                });
            }
            if let Some(target) = syms.aliases.get(seg) {
                if last {
                    return Ok(Res::Alias(m, target.clone()));
                }
                // `Alias::assoc(..)`: follow the alias when it names a type
                // directly, then continue with the rest of the path.
                let Some(mut spliced) = type_path_segs(target) else {
                    bail!("`{seg}` does not name a type with members");
                };
                spliced.extend_from_slice(&segs[i + 1..]);
                return self.resolve_at(m, &spliced, depth + 1);
            }
            if let Some(&child) = syms.children.get(seg) {
                if last {
                    return Ok(Res::Module);
                }
                m = child;
                anchored = true;
                i += 1;
                continue;
            }
            if let Some(target) = syms.uses.get(seg) {
                let mut spliced = target.clone();
                spliced.extend_from_slice(&segs[i + 1..]);
                return match self.resolve_use(m, &spliced, depth + 1)? {
                    // An import of something we do not model, `use std::fs`,
                    // stays external with the alias expanded.
                    Res::External(_) => Ok(Res::External(spliced)),
                    other => Ok(other),
                };
            }
            if anchored || m != start {
                bail!("cannot find `{seg}` in {}", module_name(syms));
            }
            return Ok(Res::External(segs[i..].to_vec()));
        }
    }

    /// Resolve a type path to a user struct canonical key, following aliases.
    pub fn resolve_struct_key(&self, m: usize, path: &syn::Path) -> Option<Rc<str>> {
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        match self.resolve(m, &segs).ok()? {
            Res::Struct(c) => Some(c),
            Res::Alias(am, target) => {
                if let syn::Type::Path(p) = &*target {
                    self.resolve_struct_key(am, &p.path)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Fail on `use ...::*` imports that point into script modules. Globs of
    /// external crates keep their old ignored behavior.
    pub fn reject_module_globs(&self) -> Result<()> {
        for (m, syms) in self.modules.iter().enumerate() {
            for prefix in &syms.globs {
                if let Ok(Res::Module) = self.resolve_use(m, prefix, 0) {
                    bail!(
                        "unsupported feature: glob import `use {}::*` of a script module",
                        prefix.join("::")
                    );
                }
            }
        }
        Ok(())
    }
}

fn module_name(syms: &ModuleSyms) -> String {
    if syms.path.is_empty() {
        "the script root".to_string()
    } else {
        format!("module `{}`", syms.path.join("::"))
    }
}

/// The plain segments of a path type, `a::b::C` without generics on the way.
fn type_path_segs(ty: &syn::Type) -> Option<Vec<String>> {
    if let syn::Type::Path(p) = ty {
        Some(p.path.segments.iter().map(|s| s.ident.to_string()).collect())
    } else {
        None
    }
}

/// Trailing name of a canonical key, what compiled Rust would print.
pub(super) fn bare(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}
