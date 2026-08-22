//! Expressions and control flow.

use anyhow::{Result, anyhow, bail};
use syn::spanned::Spanned;
use syn::{BinOp, Expr, Lit, UnOp};

use std::sync::Arc;

use crate::interpreter::bytecode::{BinKind, Const, Op, Reg, UnKind};
use crate::interpreter::numeric::{IntWidth, truncate};

use super::walks::qualified_method_ref;

use super::infer::Ty;
use super::{
    Compiler, FloatTy, NameLoc, NumericTy, bin_kind, expr_kind, int_literal, is_assign_op,
};

/// The operand type a typed op carries.
pub(super) enum Typed {
    Int(IntWidth),
    Float(bool),
}

impl Compiler<'_> {
    /// A plain local returns its own register with no copy.
    pub(super) fn compile_expr(&mut self, expr: &Expr) -> Result<Reg> {
        if let Expr::Path(p) = expr
            && p.path.segments.len() == 1
            && p.qself.is_none()
        {
            let name = p.path.segments[0].ident.to_string();
            if let NameLoc::Local(reg) = self.resolve(&name) {
                return Ok(reg);
            }
        }
        let dst = self.alloc();
        self.compile_into(dst, expr)?;
        Ok(dst)
    }

    pub(super) fn compile_into(&mut self, dst: Reg, expr: &Expr) -> Result<()> {
        self.set_line(expr.span());
        match expr {
            Expr::Lit(lit) => {
                let width = self.inferred_width(expr);
                self.compile_lit(dst, &lit.lit, width)?;
            }
            Expr::Paren(p) => self.compile_into(dst, &p.expr)?,
            Expr::Group(g) => self.compile_into(dst, &g.expr)?,
            Expr::Reference(r) => self.compile_into(dst, &r.expr)?,
            Expr::Unsafe(u) => self.compile_block(&u.block, dst)?,
            Expr::Block(b) => self.compile_block(&b.block, dst)?,
            // `<[T]>::len` as a value is a method reference
            Expr::Path(p) if p.qself.is_some() && p.path.segments.len() == 1 => {
                let segs = qualified_method_ref(p);
                self.compile_resolved_value(dst, &segs)?;
            }
            Expr::Path(p) => self.compile_path(dst, &p.path)?,
            Expr::Unary(u) => self.compile_unary(dst, u)?,
            Expr::Binary(b) => self.compile_binary(dst, b)?,
            Expr::Assign(a) => {
                self.compile_assign(&a.left, &a.right)?;
                self.emit(Op::LoadUnit { dst });
            }
            Expr::If(if_expr) => self.compile_if(dst, if_expr)?,
            Expr::While(w) => self.compile_while(dst, w)?,
            Expr::ForLoop(f) => self.compile_for(dst, f)?,
            Expr::Loop(l) => self.compile_loop(dst, l)?,
            Expr::Match(m) => self.compile_match(dst, m)?,
            Expr::Return(r) => self.compile_return(r)?,
            Expr::Break(b) => self.compile_break(b)?,
            Expr::Continue(_) => self.compile_continue()?,
            Expr::Call(c) => self.compile_call(dst, c)?,
            Expr::MethodCall(m) => self.compile_method(dst, m)?,
            Expr::Macro(m) => self.compile_macro(&m.mac, dst)?,
            Expr::Tuple(t) => {
                let base = self.compile_args(t.elems.iter())?;
                self.emit(Op::MakeTuple {
                    dst,
                    base,
                    count: u16::try_from(t.elems.len())?,
                });
            }
            Expr::Array(a) => {
                let base = self.compile_args(a.elems.iter())?;
                self.emit(Op::MakeVec {
                    dst,
                    base,
                    count: u16::try_from(a.elems.len())?,
                });
            }
            Expr::Repeat(r) => {
                let val = self.compile_expr(&r.expr)?;
                let count = self.compile_expr(&r.len)?;
                self.emit(Op::MakeArrayRepeat { dst, val, count });
            }
            Expr::Index(idx) => {
                let base = self.compile_expr(&idx.expr)?;
                let key = self.compile_expr(&idx.index)?;
                // `rustc` names the opening bracket of an indexing panic
                self.set_line(idx.bracket_token.span.open());
                self.emit(Op::Index { dst, base, key });
            }
            Expr::Field(f) => {
                let base = self.compile_expr(&f.base)?;
                let member = self.member_of(&f.member);
                self.emit(Op::GetField { dst, base, member });
            }
            Expr::Struct(s) => self.compile_struct_literal(dst, s)?,
            Expr::Range(r) => self.compile_range(dst, r)?,
            Expr::Try(t) => self.compile_try(dst, t)?,
            Expr::Cast(c) => {
                let src = self.compile_expr(&c.expr)?;
                let ty = self.add_cast(&c.ty);
                self.emit(Op::Cast { dst, src, ty });
            }
            Expr::Closure(c) => self.compile_closure(dst, c)?,
            Expr::Await(a) => {
                if !self.ctx.async_mode {
                    bail!("`.await` is only available under #[tokio::main]");
                }
                let src = self.compile_expr(&a.base)?;
                self.emit(Op::Await { dst, src });
            }
            Expr::Async(_) => {
                bail!("an async block is only supported directly inside tokio::spawn")
            }
            other => bail!("unsupported expression: {}", expr_kind(other)),
        }
        Ok(())
    }

    /// A binary op, typed when both operands are known scalars of one kind.
    pub(super) fn emit_bin(&mut self, dst: Reg, a: Reg, b: Reg, op: BinKind, typed: Option<Typed>) {
        match typed {
            Some(Typed::Int(w)) => self.emit(Op::BinInt { dst, a, b, op, w }),
            Some(Typed::Float(f32)) => self.emit(Op::BinFloat { dst, a, b, op, f32 }),
            None => self.emit(Op::Bin { dst, a, b, op }),
        }
    }

    pub(super) fn emit_bin_imm(
        &mut self,
        dst: Reg,
        a: Reg,
        imm: i64,
        op: BinKind,
        typed: Option<Typed>,
    ) {
        match typed {
            Some(Typed::Int(w)) => self.emit(Op::BinIntImm { dst, a, imm, op, w }),
            _ => self.emit(Op::BinImm { dst, a, imm, op }),
        }
    }

    /// `typed_operands` for arithmetic alone. A comparison as a value stays generic, the typed
    /// compare is a jump.
    pub(super) fn typed_arith(&self, left: &Expr, right: &Expr, op: BinKind) -> Option<Typed> {
        match op {
            BinKind::Add | BinKind::Sub | BinKind::Mul | BinKind::Div | BinKind::Rem => {
                self.typed_operands(left, right, op)
            }
            _ => None,
        }
    }

    /// The shared scalar type of both operands when the pass typed them, for the typed ops.
    /// Shifts, bit ops, float comparisons and `Str + &str` stay on the generic op.
    pub(super) fn typed_operands(&self, left: &Expr, right: &Expr, op: BinKind) -> Option<Typed> {
        let compares = matches!(
            op,
            BinKind::Eq | BinKind::Ne | BinKind::Lt | BinKind::Le | BinKind::Gt | BinKind::Ge
        );
        let arithmetic = matches!(
            op,
            BinKind::Add | BinKind::Sub | BinKind::Mul | BinKind::Div | BinKind::Rem
        );
        if !compares && !arithmetic {
            return None;
        }
        match (self.types.of(left), self.types.of(right)) {
            (Ty::Int(a), Ty::Int(b)) if a == b && !a.is_big() => Some(Typed::Int(a)),
            (Ty::F64, Ty::F64) if arithmetic => Some(Typed::Float(false)),
            (Ty::F32, Ty::F32) if arithmetic => Some(Typed::Float(true)),
            _ => None,
        }
    }

    /// The width the inference pass gave a numeric expression, `None` when it has none.
    pub(super) fn inferred_width(&self, expr: &Expr) -> Option<NumericTy> {
        match self.types.of(expr) {
            super::infer::Ty::Int(w) => Some(NumericTy::Int(w)),
            super::infer::Ty::F32 => Some(NumericTy::Float(FloatTy::F32)),
            super::infer::Ty::F64 => Some(NumericTy::Float(FloatTy::F64)),
            _ => None,
        }
    }

    pub(super) fn compile_lit(
        &mut self,
        dst: Reg,
        lit: &Lit,
        width: Option<NumericTy>,
    ) -> Result<()> {
        let int_width = match width {
            Some(NumericTy::Int(w)) => Some(w),
            _ => None,
        };
        let float_width = match width {
            Some(NumericTy::Float(f)) => Some(f),
            _ => None,
        };
        match lit {
            Lit::Int(i) => self.compile_int_lit(dst, i, false, int_width)?,
            Lit::Bool(b) => self.emit(Op::LoadBool { dst, v: b.value }),
            Lit::Float(f) => self.compile_float_lit(dst, f, false, float_width)?,
            Lit::Str(s) => {
                let k = self.add_const(Const::Str(Arc::from(s.value().as_str())));
                self.emit(Op::LoadConst { dst, k });
            }
            Lit::Char(c) => {
                let k = self.add_const(Const::Char(c.value()));
                self.emit(Op::LoadConst { dst, k });
            }
            Lit::Byte(b) => self.emit(Op::LoadInt {
                dst,
                v: i64::from(b.value()),
            }),
            Lit::ByteStr(bs) => {
                let k = self.add_const(Const::Bytes(Arc::from(bs.value().as_slice())));
                self.emit(Op::LoadConst { dst, k });
            }
            other => bail!("unsupported literal: {other:?}"),
        }
        Ok(())
    }

    /// Parses through u128 so a bare literal past `i64::MAX` keeps its value. `negated` lets `-128i8`
    /// and `-9223372036854775808` type before they could overflow.
    pub(super) fn compile_int_lit(
        &mut self,
        dst: Reg,
        lit: &syn::LitInt,
        negated: bool,
        annotation: Option<IntWidth>,
    ) -> Result<()> {
        let raw: u128 = lit.base10_parse()?;
        // a literal past u64 can only be 128 bits wide
        if raw > u128::from(u64::MAX) {
            let stated = match lit.suffix() {
                "" => annotation.unwrap_or(if negated {
                    IntWidth::I128
                } else {
                    IntWidth::U128
                }),
                other => IntWidth::parse(other)
                    .ok_or_else(|| anyhow!("unsupported literal suffix `{other}`"))?,
            };
            let value = if negated {
                if stated != IntWidth::I128 || raw > 1u128 << 127 {
                    bail!("integer literal does not fit any supported width");
                }
                // `wrapping_neg` turns 2^127 into `i128::MIN` exactly
                raw.cast_signed().wrapping_neg()
            } else {
                if !stated.is_big() || (stated == IntWidth::I128 && raw > i128::MAX.cast_unsigned())
                {
                    bail!("integer literal does not fit any supported width");
                }
                raw.cast_signed()
            };
            let k = self.add_const(Const::Big(value, stated));
            self.emit(Op::LoadConst { dst, k });
            return Ok(());
        }
        let mut value = i128::try_from(raw)?;
        if negated {
            value = -value;
        }
        let width = match lit.suffix() {
            "" => annotation,
            suffix => Some(
                IntWidth::parse(suffix)
                    .ok_or_else(|| anyhow!("unsupported literal suffix `{suffix}`"))?,
            ),
        };
        let width = width.unwrap_or({
            // untyped and past `i64::MAX` can only be u64 or usize
            if value > i128::from(i64::MAX) {
                IntWidth::U64
            } else {
                IntWidth::I64
            }
        });
        match width {
            IntWidth::I64 => self.emit(Op::LoadInt {
                dst,
                v: i64::try_from(value)?,
            }),
            w if w.is_big() => {
                let k = self.add_const(Const::Big(truncate(value, w), w));
                self.emit(Op::LoadConst { dst, k });
            }
            w => self.emit(Op::LoadIntW {
                dst,
                v: w.encode(truncate(value, w)),
                w,
            }),
        }
        Ok(())
    }

    /// An f32 parses from its own digits, never through f64 rounding.
    pub(super) fn compile_float_lit(
        &mut self,
        dst: Reg,
        lit: &syn::LitFloat,
        negated: bool,
        annotation: Option<FloatTy>,
    ) -> Result<()> {
        let is_f32 = match lit.suffix() {
            "f32" => true,
            "f64" => false,
            _ => annotation == Some(FloatTy::F32),
        };
        let k = if is_f32 {
            let mut v: f32 = lit.base10_parse()?;
            if negated {
                v = -v;
            }
            self.add_const(Const::F32(v))
        } else {
            let mut v: f64 = lit.base10_parse()?;
            if negated {
                v = -v;
            }
            self.add_const(Const::Float(v))
        };
        self.emit(Op::LoadConst { dst, k });
        Ok(())
    }

    /// With `Drop` impls the early return runs the scope drops it would skip.
    pub(super) fn compile_try(&mut self, dst: Reg, t: &syn::ExprTry) -> Result<()> {
        let src = self.compile_expr(&t.expr)?;
        let conv = self.try_conv();
        if self.ctx.has_drop {
            let site = self.here();
            self.emit(Op::TryJump {
                dst,
                src,
                to: 0,
                conv,
            });
            let depth = self.cur().scope_order.len();
            self.emit_scope_drops(depth);
            self.emit(Op::Ret { src: dst });
            let ok = self.mark()?;
            self.patch_jump(site, ok);
        } else {
            self.emit(Op::Try { dst, src, conv });
        }
        Ok(())
    }

    pub(super) fn compile_path(&mut self, dst: Reg, path: &syn::Path) -> Result<()> {
        if path.segments.len() == 1 {
            let name = path.segments[0].ident.to_string();
            return self.load_name(&name, dst);
        }
        // `Self::Unit` inside an impl names the impl's own type
        let mut segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        if segs[0] == "Self"
            && let Some(ty) = self.ctx.impl_type
        {
            segs[0] = ty.to_string();
        }
        self.compile_resolved_value(dst, &segs)
    }

    pub(super) fn compile_unary(&mut self, dst: Reg, u: &syn::ExprUnary) -> Result<()> {
        if matches!(u.op, UnOp::Deref(_)) {
            let src = self.compile_expr(&u.expr)?;
            self.emit(Op::Deref { dst, src });
            return Ok(());
        }
        // a negated literal types as 1 token, so `-128i8` loads directly
        if matches!(u.op, UnOp::Neg(_))
            && let Expr::Lit(l) = &*u.expr
        {
            let width = self.inferred_width(&u.expr);
            match (&l.lit, width) {
                (Lit::Int(i), Some(NumericTy::Int(w))) => {
                    return self.compile_int_lit(dst, i, true, Some(w));
                }
                (Lit::Int(i), _) => return self.compile_int_lit(dst, i, true, None),
                (Lit::Float(f), Some(NumericTy::Float(t))) => {
                    return self.compile_float_lit(dst, f, true, Some(t));
                }
                (Lit::Float(f), _) => return self.compile_float_lit(dst, f, true, None),
                _ => {}
            }
        }
        let a = self.compile_expr(&u.expr)?;
        let op = match u.op {
            UnOp::Neg(_) => UnKind::Neg,
            UnOp::Not(_) => UnKind::Not,
            _ => bail!("unsupported unary operator"),
        };
        self.emit(Op::Un { dst, a, op });
        Ok(())
    }

    pub(super) fn compile_binary(&mut self, dst: Reg, b: &syn::ExprBinary) -> Result<()> {
        if is_assign_op(&b.op) {
            let op = bin_kind(&b.op).ok_or_else(|| anyhow!("unsupported operator {:?}", b.op))?;
            self.compile_compound_assign(&b.left, op, &b.right)?;
            self.emit(Op::LoadUnit { dst });
            return Ok(());
        }
        match b.op {
            BinOp::And(_) => {
                self.compile_into(dst, &b.left)?;
                let jmp = self.here();
                self.emit(Op::JumpIfFalse { cond: dst, to: 0 });
                self.compile_into(dst, &b.right)?;
                let end = self.mark()?;
                self.patch_jump(jmp, end);
                return Ok(());
            }
            BinOp::Or(_) => {
                self.compile_into(dst, &b.left)?;
                let jmp = self.here();
                self.emit(Op::JumpIfTrue { cond: dst, to: 0 });
                self.compile_into(dst, &b.right)?;
                let end = self.mark()?;
                self.patch_jump(jmp, end);
                return Ok(());
            }
            _ => {}
        }
        let op = bin_kind(&b.op).ok_or_else(|| anyhow!("unsupported operator {:?}", b.op))?;
        let a = self.compile_expr(&b.left)?;
        let typed = self.typed_operands(&b.left, &b.right, op);
        // a literal immediate adopts the width of the left side like a bare literal
        if let Some(imm) = int_literal(&b.right) {
            self.set_line(b.left.span());
            self.emit_bin_imm(dst, a, imm, op, typed);
            return Ok(());
        }
        let c = self.compile_expr(&b.right)?;
        self.set_line(b.left.span());
        self.emit_bin(dst, a, c, op, typed);
        Ok(())
    }

    // statements

    /// Returns the jump index for patching. A plain comparison becomes a fused compare and branch.
    pub(super) fn emit_cond_jump(&mut self, cond: &Expr) -> Result<usize> {
        if let Expr::Binary(b) = cond
            && let Some(op) = bin_kind(&b.op)
            && !is_assign_op(&b.op)
            && matches!(
                op,
                BinKind::Eq | BinKind::Ne | BinKind::Lt | BinKind::Le | BinKind::Gt | BinKind::Ge
            )
        {
            let a = self.compile_expr(&b.left)?;
            let typed = self.typed_operands(&b.left, &b.right, op);
            if let Some(imm) = int_literal(&b.right) {
                let at = self.here();
                match typed {
                    Some(Typed::Int(w)) => self.emit(Op::CmpJumpIntImm {
                        a,
                        imm,
                        op,
                        w,
                        to: 0,
                    }),
                    _ => self.emit(Op::CmpJumpImm { a, imm, op, to: 0 }),
                }
                return Ok(at);
            }
            let c = self.compile_expr(&b.right)?;
            let at = self.here();
            match typed {
                Some(Typed::Int(w)) => self.emit(Op::CmpJumpInt {
                    a,
                    b: c,
                    op,
                    w,
                    to: 0,
                }),
                _ => self.emit(Op::CmpJump { a, b: c, op, to: 0 }),
            }
            return Ok(at);
        }
        let c = self.compile_expr(cond)?;
        let at = self.here();
        self.emit(Op::JumpIfFalse { cond: c, to: 0 });
        Ok(at)
    }

    /// An open end is an `i64::MAX` sentinel every consumer reads as "to the end", so
    /// `s.get(3..)` works like `s[3..]`.
    pub(super) fn compile_range(&mut self, dst: Reg, r: &syn::ExprRange) -> Result<()> {
        let start = if let Some(e) = &r.start {
            self.compile_expr(e)?
        } else {
            let z = self.alloc();
            self.emit(Op::LoadInt { dst: z, v: 0 });
            z
        };
        let end = if let Some(e) = &r.end {
            self.compile_expr(e)?
        } else {
            let z = self.alloc();
            self.emit(Op::LoadInt {
                dst: z,
                v: i64::MAX,
            });
            z
        };
        let inclusive = matches!(r.limits, syn::RangeLimits::Closed(_));
        self.emit(Op::MakeRange {
            dst,
            start,
            end,
            inclusive,
        });
        Ok(())
    }

    // control flow
}
