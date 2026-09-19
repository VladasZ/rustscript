//! Name resolution inside a function, locals, captures, aliases and consts.

use anyhow::{Result, bail};
use syn::{Expr, Pat};

use super::place::single_path_name;
use super::walks::unparen;
use super::{Compiler, NameLoc, idx16};
use crate::interpreter::bytecode::{CapSource, EnumVariant, Op, PathRef, Reg};
use crate::interpreter::resolver::Res;

impl Compiler<'_> {
    /// Every alias change goes through here, so the scope that made it can undo it.
    pub(super) fn set_alias(&mut self, name: &str, target: Option<String>) {
        let f = self.cur();
        let previous = match target {
            Some(target) => f.aliases.insert(name.to_string(), target),
            None => match f.aliases.remove(name) {
                Some(previous) => Some(previous),
                // nothing changed, so there is nothing to undo
                None => return,
            },
        };
        f.alias_log.push((name.to_string(), previous));
    }

    /// `ref mut m = n` over a variable. A scalar or a string register has no reference form,
    /// so the binding becomes an alias of the variable like `let m = &mut n`, and the pattern
    /// lowers as `_`. True when the alias was made.
    pub(super) fn alias_ref_binding(&mut self, pat: &Pat, scrutinee: &Expr) -> bool {
        let mut binding = pat;
        while let Pat::Type(syn::PatType { pat, .. }) | Pat::Paren(syn::PatParen { pat, .. }) =
            binding
        {
            binding = pat;
        }
        let Pat::Ident(id) = binding else {
            return false;
        };
        if id.by_ref.is_none() || id.subpat.is_some() {
            return false;
        }
        let Some(var) = single_path_name(unparen(scrutinee)) else {
            return false;
        };
        let target = self.unalias(&var);
        if matches!(self.resolve(&target), NameLoc::None) {
            return false;
        }
        self.set_alias(&id.ident.to_string(), Some(target));
        true
    }

    /// `*n` inside a closure for a `&mut` parameter `n` of an enclosing function. The
    /// parameter lives in its capture cell and the closure sees the cell as an upvalue, so
    /// the write goes to the upvalue and the function's return hands the cell back to the
    /// caller, like `deref_param_cell` does for the function body.
    pub(super) fn deref_param_upvalue(&self, name: &str) -> bool {
        let mut frames = self.frames.iter().rev();
        let Some(cur) = frames.next() else {
            return false;
        };
        if cur.aliases.contains_key(name) || cur.local_reg(name).is_some() {
            return false;
        }
        for frame in frames {
            if frame.aliases.contains_key(name) {
                return false;
            }
            if let Some(reg) = frame.local_reg(name) {
                return frame.borrow_params.contains(&reg) && (reg as usize) < frame.num_params;
            }
        }
        false
    }

    pub(super) fn resolve(&mut self, name: &str) -> NameLoc {
        let name = &self.unalias(name);
        let depth = self.frames.len() - 1;
        if let Some(reg) = self.frames[depth].local_reg(name) {
            return if self.frames[depth].mutable_locals.contains(&reg) {
                NameLoc::Cell(reg)
            } else {
                NameLoc::Local(reg)
            };
        }
        if let Some(idx) = self.frames[depth].upvalue_index(name) {
            return NameLoc::Upvalue(idx);
        }
        match self.capture(depth, name) {
            Some(idx) => NameLoc::Upvalue(idx),
            None => NameLoc::None,
        }
    }

    /// A frame that defines `name` itself shadows any outer alias.
    pub(super) fn enclosing_alias_target(&self, name: &str) -> Option<String> {
        for frame in self.frames.iter().rev().skip(1) {
            let mut seen = name;
            while let Some(next) = frame.aliases.get(seen) {
                seen = next;
            }
            if seen != name {
                return Some(seen.to_string());
            }
            if frame.local_reg(name).is_some() || frame.upvalue_index(name).is_some() {
                return None;
            }
        }
        None
    }

    /// A closure capturing `r` from `let r = &mut v` captures `v` itself.
    pub(super) fn parent_alias_target(&self, parent: usize, name: &str) -> Option<String> {
        let aliases = &self.frames[parent].aliases;
        let mut seen = aliases.get(name)?;
        while let Some(next) = aliases.get(seen) {
            seen = next;
        }
        Some(seen.clone())
    }

    pub(super) fn capture(&mut self, depth: usize, name: &str) -> Option<u16> {
        if depth == 0 {
            return None;
        }
        let parent = depth - 1;
        // writes through the alias must cross the frame boundary, so the capture is mutable
        if let Some(target) = self.parent_alias_target(parent, name) {
            return self.capture_mutable_as(depth, &target, name);
        }
        if let Some(reg) = self.frames[parent].local_reg(name) {
            let source = if self.frames[parent].mutable_locals.contains(&reg) {
                CapSource::MutableLocal(reg)
            } else {
                CapSource::Local(reg)
            };
            return Some(self.add_upvalue(depth, name, source));
        }
        if let Some(idx) = self.frames[parent].upvalue_index(name) {
            let source = if self.frames[parent].upvalues[idx as usize].1.is_mutable() {
                CapSource::MutableUpvalue(idx)
            } else {
                CapSource::Upvalue(idx)
            };
            return Some(self.add_upvalue(depth, name, source));
        }
        let idx = self.capture(parent, name)?;
        let source = if self.frames[parent].upvalues[idx as usize].1.is_mutable() {
            CapSource::MutableUpvalue(idx)
        } else {
            CapSource::Upvalue(idx)
        };
        Some(self.add_upvalue(depth, name, source))
    }

    pub(super) fn resolve_for_write(&mut self, name: &str) -> NameLoc {
        let name = &self.unalias(name);
        let depth = self.frames.len() - 1;
        if let Some(reg) = self.frames[depth].local_reg(name) {
            return if self.frames[depth].mutable_locals.contains(&reg) {
                NameLoc::Cell(reg)
            } else {
                NameLoc::Local(reg)
            };
        }
        if let Some(idx) = self.frames[depth].upvalue_index(name) {
            self.mark_upvalue_mutable(depth, idx);
            return NameLoc::Upvalue(idx);
        }
        match self.capture_mutable(depth, name) {
            Some(idx) => NameLoc::Upvalue(idx),
            None => NameLoc::None,
        }
    }

    pub(super) fn capture_mutable(&mut self, depth: usize, name: &str) -> Option<u16> {
        self.capture_mutable_as(depth, name, name)
    }

    /// The 2 names differ when an alias captures its borrowed variable.
    pub(super) fn capture_mutable_as(
        &mut self,
        depth: usize,
        name: &str,
        register_as: &str,
    ) -> Option<u16> {
        if depth == 0 {
            return None;
        }
        let parent = depth - 1;
        if let Some(target) = self.parent_alias_target(parent, name) {
            return self.capture_mutable_as(depth, &target, register_as);
        }
        if let Some(reg) = self.frames[parent].local_reg(name) {
            self.frames[parent].mutable_locals.insert(reg);
            return Some(self.add_upvalue(depth, register_as, CapSource::MutableLocal(reg)));
        }
        if let Some(idx) = self.frames[parent].upvalue_index(name) {
            self.mark_upvalue_mutable(parent, idx);
            return Some(self.add_upvalue(depth, register_as, CapSource::MutableUpvalue(idx)));
        }
        let idx = self.capture_mutable(parent, name)?;
        Some(self.add_upvalue(depth, register_as, CapSource::MutableUpvalue(idx)))
    }

    pub(super) fn mark_upvalue_mutable(&mut self, depth: usize, idx: u16) {
        let source = self.frames[depth].upvalues[idx as usize].1;
        let mutable_source = match source {
            CapSource::Local(reg) => {
                self.frames[depth - 1].mutable_locals.insert(reg);
                CapSource::MutableLocal(reg)
            }
            CapSource::Upvalue(parent_idx) => {
                self.mark_upvalue_mutable(depth - 1, parent_idx);
                CapSource::MutableUpvalue(parent_idx)
            }
            CapSource::MutableLocal(_) | CapSource::MutableUpvalue(_) => return,
        };
        self.frames[depth].upvalues[idx as usize].1 = mutable_source;
    }

    pub(super) fn add_upvalue(&mut self, depth: usize, name: &str, src: CapSource) -> u16 {
        if let Some(i) = self.frames[depth].upvalue_index(name) {
            return i;
        }
        self.frames[depth].upvalues.push((name.to_string(), src));
        idx16(self.frames[depth].upvalues.len() - 1)
    }

    pub(super) fn load_name(&mut self, name: &str, dst: Reg) -> Result<()> {
        match self.resolve(name) {
            NameLoc::Local(reg) => {
                if reg != dst {
                    self.emit(Op::Move { dst, src: reg });
                }
                Ok(())
            }
            NameLoc::Cell(cell) => {
                self.emit(Op::LoadCell { dst, cell });
                Ok(())
            }
            NameLoc::Upvalue(idx) => {
                self.emit(Op::LoadUpvalue { dst, idx });
                Ok(())
            }
            NameLoc::None => self.compile_resolved_value(dst, &[name.to_string()]),
        }
    }

    /// Consts, imported variants and unit structs resolve here, the rest is left for the VM.
    pub(super) fn compile_resolved_value(&mut self, dst: Reg, segs: &[String]) -> Result<()> {
        let resolved = self.resolve_path_res(segs)?;
        let path = match resolved {
            Res::Const(idx) => {
                self.emit(Op::LoadGlobal { dst, idx });
                return Ok(());
            }
            Res::Struct(c) | Res::Enum(c) => PathRef::user(vec![c.to_string()], None),
            Res::TypeMember(c, rest) => {
                if let Some(variant) =
                    self.enum_variant(&c, &rest, |fields| matches!(fields, syn::Fields::Unit))
                {
                    let info = self.add_enum_variant(variant);
                    self.emit(Op::LoadEnum { dst, info });
                    return Ok(());
                }
                // `S::LIMIT` lives in the impl's module, which may not be the module using it
                if rest.len() == 1 {
                    let key = format!("{}::{}", crate::interpreter::resolver::bare(&c), rest[0]);
                    let found = self
                        .ctx
                        .resolver
                        .modules
                        .iter()
                        .find_map(|syms| syms.consts.get(&key).copied());
                    if let Some(idx) = found {
                        self.emit(Op::LoadGlobal { dst, idx });
                        return Ok(());
                    }
                }
                let mut segs = vec![c.to_string()];
                segs.extend(rest);
                PathRef::user(segs, None)
            }
            Res::Alias(m, target) => {
                let path = match &*target {
                    syn::Type::Path(p) => p.path.clone(),
                    _ => bail!("`{}` does not name a value", segs.join("::")),
                };
                match self.ctx.resolver.resolve_struct_key(m, &path) {
                    Some(c) => PathRef::user(vec![c.to_string()], None),
                    None => bail!("`{}` does not name a value", segs.join("::")),
                }
            }
            Res::Module => bail!("`{}` is a module, not a value", segs.join("::")),
            Res::External(canon) => {
                // builtin unit variants load in place like a user variant
                if let Some((def, index)) = self.resolve_variant(segs)
                    && def.is_unit(index)
                {
                    let info = self.add_enum_variant(EnumVariant {
                        def,
                        variant: index,
                    });
                    self.emit(Op::LoadEnum { dst, info });
                    return Ok(());
                }
                self.external_path(canon, None)
            }
            Res::Fn(_) => PathRef::user(segs.to_vec(), None),
        };
        let path = self.add_path(path);
        self.emit(Op::PathValue { dst, path });
        Ok(())
    }

    // blocks and statements
}
