//! Module aware name resolution. Every item gets a canonical key like `foo::bar`, a bare `bar` at
//! the root. Anything that never lands on a user item falls through to the bridge dispatch.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Result, bail};

use super::bytecode::NO_TYPE;
use super::enum_def::EnumDef;

#[derive(Default, Clone)]
pub(super) struct ModuleSyms {
    pub path: Vec<String>,
    pub parent: Option<usize>,
    /// `crate::` stops here and `super` stops here even with a tree parent
    pub crate_root: bool,
    pub children: HashMap<String, usize>,
    pub fns: HashMap<String, u32>,
    pub consts: HashMap<String, u32>,
    pub structs: HashMap<String, Arc<str>>,
    pub enums: HashMap<String, Arc<str>>,
    pub aliases: HashMap<String, Rc<syn::Type>>,
    pub uses: HashMap<String, Vec<String>>,
    /// the prefix of every `use prefix::*`, one that names a script module brings its items in
    pub globs: Vec<Vec<String>>,
    /// Set on the copy a fn body with a `use` compiles against, the module the fn is written in.
    pub scope_of: Option<usize>,
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
    /// `Type::rest` on a user type
    TypeMember(Arc<str>, Vec<String>),
    /// resolved in its defining module
    Alias(usize, Rc<syn::Type>),
    Module,
    /// segments have imports already expanded
    External(Vec<String>),
}

pub(super) struct Resolver {
    pub modules: Vec<ModuleSyms>,
    pub structs: HashMap<Arc<str>, StructDef>,
    pub enums: HashMap<Arc<str>, Rc<syn::ItemEnum>>,
    pub enum_defs: HashMap<Arc<str>, Arc<EnumDef>>,
    /// Handed out to every declared type and every other impl target like `impl MyTrait for PathBuf`.
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

    /// The canonical path of a fn, the key the fn index holds it under.
    pub fn fn_segs(&self, idx: u32) -> Option<Vec<String>> {
        self.modules.iter().find_map(|syms| {
            let (name, _) = syms.fns.iter().find(|(_, f)| **f == idx)?;
            let mut segs = syms.path.clone();
            segs.push(name.clone());
            Some(segs)
        })
    }

    pub fn resolve(&self, m: usize, segs: &[String]) -> Result<Res> {
        self.resolve_at(m, segs, 0)
    }

    /// `use ctx::Ctx` next to `mod ctx` resolves locally in `rustc`, so a submodule tries itself
    /// first and falls back to the crate root.
    fn resolve_use(&self, m: usize, segs: &[String], depth: usize) -> Result<Res> {
        if let Some("self" | "super" | "crate") = segs.first().map(String::as_str) {
            return self.resolve_at(m, segs, depth);
        }
        // at the crate root both resolutions are the same walk, so the retry would repeat the
        // alias expansion and blow up
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
        // a leading `crate`, `self` or `super` run pins the start and turns external fallback off
        let mut anchored = false;
        while i < segs.len() {
            match segs[i].as_str() {
                "crate" => m = self.crate_root_of(m),
                "self" => {}
                "super" => {
                    // `super` may not cross a grafted crate root
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
                // `Alias::assoc(..)` follows the alias
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
                // `use which::which` names itself, expanding the import again would chase its own tail
                if target.first() == Some(seg) {
                    let external = if last { spliced } else { segs[i..].to_vec() };
                    return Ok(Res::External(external));
                }
                return match self.resolve_use(m, &spliced, depth + 1)? {
                    // `use std::fs` stays external with the alias expanded
                    Res::External(_) => Ok(Res::External(spliced)),
                    other => Ok(other),
                };
            }
            // a name the module and its imports do not have may come through a glob import
            if let Some(owner) = self.glob_owner(m, seg) {
                m = owner;
                continue;
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

    /// The module that declares `name` among the modules `m` imports by glob, `use m::*` and
    /// `use super::*`. A glob of a glob counts, and 2 modules that glob each other end the walk.
    /// A glob of an external crate names no script module and is skipped.
    fn glob_owner(&self, m: usize, name: &str) -> Option<usize> {
        let mut seen = vec![m];
        let mut queue = vec![m];
        while let Some(at) = queue.pop() {
            for prefix in &self.modules[at].globs {
                let Some(target) = self.module_index(at, prefix, 0) else {
                    continue;
                };
                if seen.contains(&target) {
                    continue;
                }
                seen.push(target);
                if self.modules[target].declares(name) {
                    return Some(target);
                }
                queue.push(target);
            }
        }
        None
    }

    /// The script module a `use` prefix names, read from module `m`.
    fn module_index(&self, mut m: usize, segs: &[String], depth: usize) -> Option<usize> {
        if depth > MAX_DEPTH {
            return None;
        }
        for (i, seg) in segs.iter().enumerate() {
            let syms = &self.modules[m];
            m = match seg.as_str() {
                "crate" => self.crate_root_of(m),
                "self" => m,
                "super" => match syms.parent {
                    Some(parent) if !syms.crate_root => parent,
                    _ => return None,
                },
                name => {
                    if let Some(&child) = syms.children.get(name) {
                        child
                    } else if let Some(target) = syms.uses.get(name) {
                        if target.first() == Some(seg) {
                            return None;
                        }
                        let mut spliced = target.clone();
                        spliced.extend_from_slice(&segs[i + 1..]);
                        return self.module_index(m, &spliced, depth + 1);
                    } else if i == 0 && m != 0 {
                        // like `resolve_use`, a path that does not start here starts at the root
                        return self.module_index(0, segs, depth + 1);
                    } else {
                        return None;
                    }
                }
            };
        }
        Some(m)
    }
}

impl ModuleSyms {
    /// Whether the module itself has an item, a child module or an import of this name.
    fn declares(&self, name: &str) -> bool {
        self.fns.contains_key(name)
            || self.consts.contains_key(name)
            || self.structs.contains_key(name)
            || self.enums.contains_key(name)
            || self.aliases.contains_key(name)
            || self.children.contains_key(name)
            || self.uses.contains_key(name)
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
