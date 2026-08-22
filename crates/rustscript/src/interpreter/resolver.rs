//! Module aware name resolution. Every item gets a canonical key like
//! `foo::bar`, a bare `bar` at the root. Anything that never lands on a user
//! item falls through to the bridge dispatch.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Result, bail};

use super::bytecode::NO_TYPE;
use super::enum_def::EnumDef;

#[derive(Default)]
pub(super) struct ModuleSyms {
    pub path: Vec<String>,
    pub parent: Option<usize>,
    /// `crate::` pins here and `super` stops here even with a tree parent.
    pub crate_root: bool,
    pub children: HashMap<String, usize>,
    pub fns: HashMap<String, u32>,
    pub consts: HashMap<String, u32>,
    pub structs: HashMap<String, Arc<str>>,
    pub enums: HashMap<String, Arc<str>>,
    pub aliases: HashMap<String, Rc<syn::Type>>,
    pub uses: HashMap<String, Vec<String>>,
    /// Checked against user modules at load.
    pub globs: Vec<Vec<String>>,
}

pub(super) struct StructDef {
    pub ast: Rc<syn::ItemStruct>,
    pub module: usize,
}

pub(super) enum Res {
    Fn(u32),
    Const(u32),
    Struct(Arc<str>),
    Enum(Arc<str>),
    /// `Type::rest` on a user type.
    TypeMember(Arc<str>, Vec<String>),
    /// Resolved in its defining module.
    Alias(usize, Rc<syn::Type>),
    Module,
    /// Segments have imports already expanded.
    External(Vec<String>),
}

pub(super) struct Resolver {
    pub modules: Vec<ModuleSyms>,
    pub structs: HashMap<Arc<str>, StructDef>,
    pub enums: HashMap<Arc<str>, Rc<syn::ItemEnum>>,
    pub enum_defs: HashMap<Arc<str>, Arc<EnumDef>>,
    /// Handed out to every declared type and every other impl target like
    /// `impl MyTrait for PathBuf`.
    pub type_ids: HashMap<Arc<str>, u16>,
}

/// So `pub use` cycles error instead of hanging.
const MAX_DEPTH: usize = 64;

impl Resolver {
    /// Hands out the next id on first sight.
    pub fn type_id(&mut self, name: &str) -> u16 {
        if let Some(id) = self.type_ids.get(name) {
            return *id;
        }
        let id = u16::try_from(self.type_ids.len()).expect("type count fits u16");
        self.type_ids.insert(Arc::from(name), id);
        id
    }

    /// `NO_TYPE` when the program never mentions the type.
    pub fn type_id_of(&self, name: &str) -> u16 {
        self.type_ids.get(name).copied().unwrap_or(NO_TYPE)
    }

    pub fn canon(&self, m: usize, name: &str) -> String {
        let path = &self.modules[m].path;
        if path.is_empty() {
            name.to_string()
        } else {
            format!("{}::{name}", path.join("::"))
        }
    }

    pub fn resolve(&self, m: usize, segs: &[String]) -> Result<Res> {
        self.resolve_at(m, segs, 0)
    }

    /// `use ctx::Ctx` beside `mod ctx` resolves locally in `rustc`, so a
    /// submodule tries itself first and falls back to the crate root.
    fn resolve_use(&self, m: usize, segs: &[String], depth: usize) -> Result<Res> {
        if let Some("self" | "super" | "crate") = segs.first().map(String::as_str) {
            return self.resolve_at(m, segs, depth);
        }
        // At the crate root both resolutions are the same walk, so the retry
        // would repeat the alias expansion and blow up.
        if m != 0
            && let Ok(res) = self.resolve_at(m, segs, depth)
            && !matches!(res, Res::External(_))
        {
            return Ok(res);
        }
        self.resolve_at(0, segs, depth)
    }

    /// The grafted root of a path dependency, or the script root.
    fn crate_root_of(&self, mut m: usize) -> usize {
        while !self.modules[m].crate_root {
            match self.modules[m].parent {
                Some(p) => m = p,
                None => break,
            }
        }
        m
    }

    fn resolve_at(&self, mut m: usize, segs: &[String], depth: usize) -> Result<Res> {
        if depth > MAX_DEPTH {
            bail!("import chain too deep resolving `{}`", segs.join("::"));
        }
        let mut i = 0;
        // A leading `crate`, `self` or `super` run pins the start and turns
        // external fallback off.
        let mut anchored = false;
        while i < segs.len() {
            match segs[i].as_str() {
                "crate" => m = self.crate_root_of(m),
                "self" => {}
                "super" => {
                    // `super` may not cross a grafted crate root.
                    m = match self.modules[m].parent {
                        Some(p) if !self.modules[m].crate_root => p,
                        _ => bail!("`super` used at the crate root"),
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
                // `Alias::assoc(..)` follows the alias.
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
                // `use which::which` names itself, expanding the import again
                // would chase its own tail.
                if target.first() == Some(seg) {
                    let external = if last { spliced } else { segs[i..].to_vec() };
                    return Ok(Res::External(external));
                }
                return match self.resolve_use(m, &spliced, depth + 1)? {
                    // `use std::fs` stays external with the alias expanded.
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

    /// Follows aliases.
    pub fn resolve_struct_key(&self, m: usize, path: &syn::Path) -> Option<Arc<str>> {
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

    /// Globs of external crates stay ignored.
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

fn type_path_segs(ty: &syn::Type) -> Option<Vec<String>> {
    if let syn::Type::Path(p) = ty {
        Some(
            p.path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect(),
        )
    } else {
        None
    }
}

/// What compiled Rust would print.
pub(super) fn bare(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}
