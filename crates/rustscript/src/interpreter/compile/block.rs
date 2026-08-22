//! Blocks, statements and `let`.

use anyhow::{Result, bail};
use syn::spanned::Spanned;
use syn::{Block, Expr, Pat, Stmt};

use crate::interpreter::bytecode::{DISCARD, Op, Reg};

use super::support::{init_is_owned, init_is_unique, pattern_borrows, pattern_owns};
use super::walks::{from_str_root, unparen};
use super::{Compiler, macro_yields_value, numeric_annotation};

impl Compiler<'_> {
    pub(super) fn compile_block(&mut self, block: &Block, dst: Reg) -> Result<()> {
        self.push_scope();
        let res = self.compile_block_inner(block, dst);
        // the block value already moved into `dst`, so a returned binding reads as shared and is
        // not dropped here
        if res.is_ok() {
            self.emit_scope_drops(1);
        }
        self.pop_scope();
        res
    }

    pub(super) fn compile_block_inner(&mut self, block: &Block, dst: Reg) -> Result<()> {
        if block.stmts.is_empty() {
            self.emit(Op::LoadUnit { dst });
            return Ok(());
        }
        // Block consts and statics bind up front like item hoisting. Their inits are const, so
        // the order is unobservable.
        for stmt in &block.stmts {
            let Stmt::Item(item) = stmt else { continue };
            match item {
                syn::Item::Const(c) => {
                    self.set_line(c.span());
                    let val = self.alloc();
                    self.compile_into(val, &c.expr)?;
                    self.define(&c.ident.to_string(), val);
                }
                syn::Item::Static(s) => {
                    if matches!(s.mutability, syn::StaticMutability::Mut(_)) {
                        bail!("unsupported feature: `static mut`");
                    }
                    self.set_line(s.span());
                    let val = self.alloc();
                    self.compile_into(val, &s.expr)?;
                    self.define(&s.ident.to_string(), val);
                }
                _ => {}
            }
        }
        let last = block.stmts.len() - 1;
        for (i, stmt) in block.stmts.iter().enumerate() {
            let is_last = i == last;
            self.set_line(stmt.span());
            let guard_mark = self.cur().guard_temps.len();
            match stmt {
                Stmt::Local(local)
                    if local
                        .init
                        .as_ref()
                        .and_then(|i| i.diverge.as_ref())
                        .is_some() =>
                {
                    self.compile_let_else(local, dst, is_last)?;
                }
                Stmt::Local(local) => {
                    self.compile_let(local, dst, is_last)?;
                }
                Stmt::Expr(expr, semi) => {
                    if is_last && semi.is_none() {
                        self.compile_owned_into(dst, expr)?;
                    } else {
                        // a statement position call discards its result
                        if let Expr::MethodCall(m) = expr {
                            self.compile_method(DISCARD, m)?;
                        } else {
                            let tmp = self.alloc();
                            self.compile_into(tmp, expr)?;
                        }
                        if is_last {
                            self.emit(Op::LoadUnit { dst });
                        }
                    }
                }
                Stmt::Item(item) => {
                    if let syn::Item::Fn(_) = item {
                        bail!("unsupported feature: nested functions");
                    }
                    if is_last {
                        self.emit(Op::LoadUnit { dst });
                    }
                }
                Stmt::Macro(m) => {
                    let target = if is_last { dst } else { self.alloc() };
                    self.compile_macro(&m.mac, target)?;
                    if is_last && !macro_yields_value(&m.mac) {
                        self.emit(Op::LoadUnit { dst });
                    }
                }
            }
            self.release_guard_temps(guard_mark, is_last.then_some(dst));
        }
        Ok(())
    }

    /// `let PAT = EXPR else { .. }`. Bindings land in the current scope.
    pub(super) fn compile_let_else(
        &mut self,
        local: &syn::Local,
        dst: Reg,
        is_last: bool,
    ) -> Result<()> {
        let init = local.init.as_ref().unwrap();
        let else_expr = &init.diverge.as_ref().unwrap().1;
        let val = self.alloc();
        let owned = pattern_owns(&local.pat) && !pattern_borrows(&local.pat);
        if owned {
            self.compile_owned_into(val, &init.expr)?;
        } else {
            self.compile_into(val, &init.expr)?;
        }
        let matched = self.alloc();
        let pidx = self.pattern_info(&local.pat)?;
        if !owned || !init_is_owned(&init.expr) {
            self.exempt_pattern_binds(pidx);
        }
        self.emit(Op::TestBind {
            val,
            pat: pidx,
            dst: matched,
        });
        let jmp_ok = self.here();
        self.emit(Op::JumpIfTrue {
            cond: matched,
            to: 0,
        });
        let else_dst = self.alloc();
        self.compile_into(else_dst, else_expr)?;
        let ok_at = self.mark()?;
        self.patch_jump(jmp_ok, ok_at);
        if is_last {
            self.emit(Op::LoadUnit { dst });
        }
        Ok(())
    }

    pub(super) fn compile_let(
        &mut self,
        local: &syn::Local,
        dst: Reg,
        is_last: bool,
    ) -> Result<()> {
        // `let r = &mut v` becomes a name alias, a projection borrow builds a reference value
        // into the element or field
        if self.compile_let_borrow(local, dst, is_last)? {
            return Ok(());
        }
        let val = self.alloc();
        match &local.init {
            Some(init) => self.compile_owned_into(val, &init.expr)?,
            None => self.emit(Op::LoadUnit { dst: val }),
        }
        self.copy_mut_binding(local, val);
        // a `from_str` init already parsed into the annotated type, see `call_coerce`
        let parsed = local
            .init
            .as_ref()
            .is_some_and(|init| from_str_root(&init.expr).is_some());
        self.bind_let(local, val, parsed)?;
        if is_last {
            self.emit(Op::LoadUnit { dst });
        }
        Ok(())
    }

    /// A `let mut` of a value that may share storage with something live copies first, so its
    /// mutations stay its own. A type that can't be `Copy` never shares.
    fn copy_mut_binding(&mut self, local: &syn::Local, val: Reg) {
        let (binding, annotation) = match &local.pat {
            Pat::Type(t) => (&*t.pat, Some(&*t.ty)),
            other => (other, None),
        };
        if let Pat::Ident(id) = binding
            && id.mutability.is_some()
            && let Some(init) = &local.init
            && !init_is_unique(&init.expr)
            && !self.is_non_copy_annotation(annotation)
        {
            self.emit(Op::Copy { dst: val, src: val });
        }
    }

    /// Binds the pattern and records which bindings own their value, so scope end drops only
    /// those.
    fn bind_let(&mut self, local: &syn::Local, val: Reg, parsed: bool) -> Result<()> {
        let before = self.cur().scope_order.last().map_or(0, Vec::len);
        if let Pat::Type(t) = &local.pat {
            if !parsed {
                self.emit_annotation(val, &t.ty);
            }
            self.bind_pattern_irrefutable(&t.pat, val)?;
        } else {
            self.bind_pattern_irrefutable(&local.pat, val)?;
        }
        if let Some(init) = &local.init {
            self.note_guard_binding(&init.expr, before);
        }
        let owned = local
            .init
            .as_ref()
            .is_none_or(|init| init_is_owned(&init.expr));
        let borrowed = local
            .init
            .as_ref()
            .is_some_and(|init| matches!(unparen(&init.expr), Expr::Reference(_)));
        if !owned || borrowed {
            let bound: Vec<Reg> = self
                .cur()
                .scope_order
                .last()
                .map_or(Vec::new(), |regs| regs[before..].to_vec());
            let f = self.cur();
            if borrowed {
                f.ref_locals.extend(bound.iter().copied());
            }
            f.drop_exempt.extend(bound);
        }
        Ok(())
    }

    /// A numeric primitive retags through a cast, which only ever acts on a bare literal.
    /// Everything else goes through the struct coercion.
    pub(super) fn emit_annotation(&mut self, reg: Reg, ty: &syn::Type) {
        if numeric_annotation(ty).is_some() {
            let idx = self.add_cast(ty);
            self.emit(Op::Cast {
                dst: reg,
                src: reg,
                ty: idx,
            });
            return;
        }
        self.emit_coerce(reg, ty);
    }

    /// A type a coercion can never change emits nothing, so annotated lets in hot loops carry no
    /// runtime work.
    pub(super) fn emit_coerce(&mut self, reg: Reg, ty: &syn::Type) {
        let ir = self.lower_ir(ty);
        if !ir.is_active() {
            return;
        }
        let idx = self.add_coerce(ir);
        self.emit(Op::Coerce {
            dst: reg,
            src: reg,
            ty: idx,
        });
    }

    // expressions
}
