//! Plain and compound assignment.

use anyhow::{Result, bail};
use syn::spanned::Spanned;
use syn::{Expr, UnOp};

use crate::interpreter::bytecode::{BinKind, FieldName, Member, Op, Reg};

use super::place;
use super::{Compiler, NameLoc, int_literal};

impl Compiler<'_> {
    /// `*seq` for a `seq: &mut usize` parameter. A cell promoted or captured name keeps the strict op.
    pub(super) fn deref_param_reg(&self, expr: &Expr) -> Option<Reg> {
        let Expr::Path(p) = expr else { return None };
        if p.qself.is_some() || p.path.segments.len() != 1 {
            return None;
        }
        let name = p.path.segments[0].ident.to_string();
        let frame = self.frames.last()?;
        let reg = frame.local_reg(&name)?;
        if frame.mutable_locals.contains(&reg) {
            return None;
        }
        ((reg as usize) < frame.num_params).then_some(reg)
    }

    /// The value stored into a local, owned. Its literals carry the local's width from the
    /// inference pass.
    pub(super) fn compile_stored_value(&mut self, value: &Expr) -> Result<Reg> {
        self.compile_owned_expr(value)
    }

    pub(super) fn compile_assign(&mut self, target: &Expr, value: &Expr) -> Result<()> {
        match target {
            Expr::Path(p) if p.path.segments.len() == 1 => {
                let name = p.path.segments[0].ident.to_string();
                let location = self.resolve_for_write(&name);
                let value = self.compile_stored_value(value)?;
                self.emit_name_store(location, value, &name)?;
            }
            Expr::Index(idx) => {
                let val = self.compile_owned_expr(value)?;
                let base = self.compile_place_base(&idx.expr)?;
                let key = self.compile_expr(&idx.index)?;
                self.set_line(idx.bracket_token.span.open());
                self.emit(Op::SetIndex { base, key, val });
            }
            Expr::Field(f) => {
                let val = self.compile_owned_expr(value)?;
                let base = self.compile_place_base(&f.base)?;
                let member = self.member_of(&f.member);
                self.emit(Op::SetField { base, member, val });
            }
            Expr::Unary(u) if matches!(u.op, UnOp::Deref(_)) => {
                // `*r = v` on a `&mut variable` alias writes the variable, which may live in an
                // enclosing frame
                if let Some(name) = place::single_path_name(&u.expr) {
                    let target = match self.unalias(&name) {
                        same if same == name => self.enclosing_alias_target(&name),
                        target => Some(target),
                    };
                    if let Some(target) = target {
                        let location = self.resolve_for_write(&target);
                        let val = self.compile_stored_value(value)?;
                        self.emit_name_store(location, val, &target)?;
                        return Ok(());
                    }
                }
                let val = self.compile_owned_expr(value)?;
                if let Some(target) = self.deref_param_reg(&u.expr) {
                    self.emit(Op::SetDerefParam { target, val });
                } else {
                    let target = self.compile_expr(&u.expr)?;
                    self.emit(Op::SetDeref { target, val });
                }
            }
            Expr::Paren(p) => self.compile_assign(&p.expr, value)?,
            _ => bail!("invalid assignment target"),
        }
        Ok(())
    }

    /// The right operand evaluates before the place, so its panic fires first.
    pub(super) fn compile_compound_assign(
        &mut self,
        target: &Expr,
        op: BinKind,
        rhs: &Expr,
    ) -> Result<()> {
        match target {
            Expr::Path(p) if p.path.segments.len() == 1 => {
                let name = p.path.segments[0].ident.to_string();
                let location = self.resolve_for_write(&name);
                let typed = self.typed_arith(target, rhs, op);
                if let Some(imm) = int_literal(rhs) {
                    let current = self.load_name_location(location, &name)?;
                    let result = self.alloc();
                    self.set_line(target.span());
                    self.emit_bin_imm(result, current, imm, op, typed);
                    self.emit_name_store(location, result, &name)?;
                } else {
                    let b = self.compile_expr(rhs)?;
                    let current = self.load_name_location(location, &name)?;
                    let result = self.alloc();
                    self.set_line(target.span());
                    self.emit_bin(result, current, b, op, typed);
                    self.emit_name_store(location, result, &name)?;
                }
            }
            Expr::Index(idx) => {
                let b = self.compile_expr(rhs)?;
                let base = self.compile_place_base(&idx.expr)?;
                let key = self.compile_expr(&idx.index)?;
                let cur = self.alloc();
                self.emit(Op::Index {
                    dst: cur,
                    base,
                    key,
                });
                let res = self.alloc();
                self.set_line(target.span());
                let typed = self.typed_arith(target, rhs, op);
                self.emit_bin(res, cur, b, op, typed);
                self.emit(Op::SetIndex {
                    base,
                    key,
                    val: res,
                });
            }
            Expr::Field(f) => {
                let b = self.compile_expr(rhs)?;
                let base = self.compile_place_base(&f.base)?;
                let member = self.member_of(&f.member);
                let cur = self.alloc();
                self.emit(Op::GetField {
                    dst: cur,
                    base,
                    member,
                });
                let res = self.alloc();
                self.set_line(target.span());
                let typed = self.typed_arith(target, rhs, op);
                self.emit_bin(res, cur, b, op, typed);
                self.emit(Op::SetField {
                    base,
                    member,
                    val: res,
                });
            }
            Expr::Unary(u) if matches!(u.op, UnOp::Deref(_)) => {
                self.compile_compound_deref_assign(u, op, rhs)?;
            }
            _ => bail!("invalid compound assignment target"),
        }
        Ok(())
    }

    /// The deref arm of `compile_compound_assign`.
    pub(super) fn compile_compound_deref_assign(
        &mut self,
        u: &syn::ExprUnary,
        op: BinKind,
        rhs: &Expr,
    ) -> Result<()> {
        // a `&mut variable` alias reads and writes the variable itself
        if let Some(name) = place::single_path_name(&u.expr) {
            let target = match self.unalias(&name) {
                // a captured alias lives in an enclosing frame
                same if same == name => self.enclosing_alias_target(&name),
                target => Some(target),
            };
            if let Some(target) = target {
                let b = self.compile_expr(rhs)?;
                let location = self.resolve_for_write(&target);
                let current = self.load_name_location(location, &target)?;
                let result = self.alloc();
                self.set_line(u.span());
                self.emit(Op::Bin {
                    dst: result,
                    a: current,
                    b,
                    op,
                });
                self.emit_name_store(location, result, &target)?;
                return Ok(());
            }
        }
        let b = self.compile_expr(rhs)?;
        let param = self.deref_param_reg(&u.expr);
        let target = self.compile_expr(&u.expr)?;
        self.set_line(u.span());
        let Some(target) = param else {
            // the fused op holds the lock across the read-modify-write, so concurrent tasks can't
            // lose updates
            self.emit(Op::DerefBinAssign { target, val: b, op });
            return Ok(());
        };
        let current = self.alloc();
        self.emit(Op::Deref {
            dst: current,
            src: target,
        });
        let result = self.alloc();
        self.emit(Op::Bin {
            dst: result,
            a: current,
            b,
            op,
        });
        self.emit(Op::SetDerefParam {
            target,
            val: result,
        });
        Ok(())
    }

    pub(super) fn load_name_location(&mut self, location: NameLoc, name: &str) -> Result<Reg> {
        match location {
            NameLoc::Local(reg) => Ok(reg),
            NameLoc::Cell(cell) => {
                let reg = self.alloc();
                self.emit(Op::LoadCell { dst: reg, cell });
                Ok(reg)
            }
            NameLoc::Upvalue(idx) => {
                let reg = self.alloc();
                self.emit(Op::LoadUpvalue { dst: reg, idx });
                Ok(reg)
            }
            NameLoc::None => bail!("assignment to unknown variable `{name}`"),
        }
    }

    pub(super) fn emit_name_store(
        &mut self,
        location: NameLoc,
        src: Reg,
        name: &str,
    ) -> Result<()> {
        match location {
            NameLoc::Local(dst) if dst != src => self.emit(Op::Move { dst, src }),
            NameLoc::Local(_) => {}
            NameLoc::Cell(cell) => self.emit(Op::StoreCell { cell, src }),
            NameLoc::Upvalue(idx) => self.emit(Op::StoreUpvalue { idx, src }),
            NameLoc::None => bail!("assignment to unknown variable `{name}`"),
        }
        Ok(())
    }

    pub(super) fn member_of(&mut self, member: &syn::Member) -> u16 {
        match member {
            syn::Member::Named(n) => {
                self.add_member(Member::Named(FieldName::new(n.to_string().into())))
            }
            syn::Member::Unnamed(i) => self.add_member(Member::Indexed(i.index as usize)),
        }
    }
}
