//! Expressions and control flow. Split from the compiler.

use anyhow::{Result, anyhow, bail};
use syn::spanned::Spanned;
use syn::{BinOp, Block, Expr, Lit, Pat, Stmt, UnOp};

use std::rc::Rc;
use std::sync::Arc;

use crate::interpreter::bytecode::{BinKind, Const, DISCARD, Op, Reg, UnKind};
use crate::interpreter::numeric::{IntWidth, truncate};

use super::*;

/// Flatten a left-nested `&&` chain into its terms, in source order. A cond
/// that is not an `&&` is returned as a single term. Used to compile let-chains
/// in `if let A = x && cond && let B = y`.
fn flatten_and(cond: &Expr) -> Vec<&Expr> {
    fn walk<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
        if let Expr::Binary(b) = e
            && matches!(b.op, BinOp::And(_))
        {
            walk(&b.left, out);
            walk(&b.right, out);
        } else {
            out.push(e);
        }
    }
    let mut out = Vec::new();
    walk(cond, &mut out);
    out
}

/// The `from_str` call at the root of a `let` init chain, looking through
/// `?`, `unwrap`, and `expect`. Only a call without its own turbofish counts.
fn from_str_root(e: &Expr) -> Option<&syn::ExprCall> {
    match e {
        Expr::Call(c) => {
            let Expr::Path(p) = &*c.func else { return None };
            let seg = p.path.segments.last()?;
            if seg.ident != "from_str" || first_generic_type(seg).is_some() {
                return None;
            }
            Some(c)
        }
        Expr::Try(t) => from_str_root(&t.expr),
        Expr::Paren(p) => from_str_root(&p.expr),
        Expr::Group(g) => from_str_root(&g.expr),
        Expr::MethodCall(m) if m.method == "unwrap" || m.method == "expect" => {
            from_str_root(&m.receiver)
        }
        _ => None,
    }
}

/// The `collect` call at the root of a `let` init chain. Only a call without
/// its own turbofish counts, a turbofish already names the target itself.
fn collect_root(e: &Expr) -> Option<&syn::ExprMethodCall> {
    match e {
        Expr::MethodCall(m) if m.method == "collect" && m.turbofish.is_none() => Some(m),
        Expr::Paren(p) => collect_root(&p.expr),
        Expr::Group(g) => collect_root(&g.expr),
        _ => None,
    }
}

/// Whether an annotated type is a plain `String`.
fn is_string_type(ty: &syn::Type) -> bool {
    matches!(ty, syn::Type::Path(p)
        if p.path.segments.last().is_some_and(|s| s.ident == "String"))
}

impl Compiler<'_> {
    pub(super) fn compile_block(&mut self, block: &Block, dst: Reg) -> Result<()> {
        self.push_scope();
        let res = self.compile_block_inner(block, dst);
        self.pop_scope();
        res
    }

    pub(super) fn compile_block_inner(&mut self, block: &Block, dst: Reg) -> Result<()> {
        if block.stmts.is_empty() {
            self.emit(Op::LoadUnit { dst });
            return Ok(());
        }
        let last = block.stmts.len() - 1;
        for (i, stmt) in block.stmts.iter().enumerate() {
            let is_last = i == last;
            self.set_line(stmt.span());
            match stmt {
                Stmt::Local(local)
                    if local
                        .init
                        .as_ref()
                        .and_then(|i| i.diverge.as_ref())
                        .is_some() =>
                {
                    // `let PAT = EXPR else { .. }`. Test the refutable pattern,
                    // and run the diverging else block when it does not match.
                    // Bindings land in the current scope, visible afterwards.
                    let init = local.init.as_ref().unwrap();
                    let else_expr = &init.diverge.as_ref().unwrap().1;
                    let val = self.alloc();
                    self.compile_into(val, &init.expr)?;
                    let matched = self.alloc();
                    let pidx = self.pattern_info(&local.pat)?;
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
                    let ok_at = self.here() as u32;
                    self.patch_jump(jmp_ok, ok_at);
                    if is_last {
                        self.emit(Op::LoadUnit { dst });
                    }
                }
                Stmt::Local(local) => {
                    let val = self.alloc();
                    // An annotated `let` whose init chain roots in a
                    // `from_str` call hands its type to that call, so the
                    // parse is typed at the source and no coerce op is
                    // needed afterwards.
                    let mut offered = false;
                    // A let nested in the init chain, say in a closure body,
                    // runs this code again before the outer collect consumes
                    // its hint, so the outer hint is restored, not cleared.
                    let outer_string_let = self.string_let.take();
                    if let Pat::Type(t) = &local.pat
                        && let Some(init) = &local.init
                    {
                        if let Some(call) = from_str_root(&init.expr) {
                            self.json_let = Some((call as *const _, Rc::new((*t.ty).clone())));
                            offered = true;
                        } else if is_string_type(&t.ty)
                            && let Some(mc) = collect_root(&init.expr)
                        {
                            self.string_let = Some(mc as *const _);
                        }
                    }
                    // A numeric annotation types a bare literal init at
                    // compile time, so the value never exists at the wrong
                    // width.
                    let mut typed_literal = false;
                    if let Pat::Type(t) = &local.pat
                        && let Some(init) = &local.init
                        && let Some(target) = numeric_annotation(&t.ty)
                    {
                        typed_literal = self.compile_numeric_annotated(val, &init.expr, target)?;
                    }
                    if !typed_literal {
                        match &local.init {
                            Some(init) => self.compile_into(val, &init.expr)?,
                            None => self.emit(Op::LoadUnit { dst: val }),
                        }
                    }
                    let consumed = offered && self.json_let.is_none();
                    self.json_let = None;
                    self.string_let = outer_string_let;
                    // A type annotation coerces a dynamic value into that type.
                    if let Pat::Type(t) = &local.pat {
                        if !consumed && !typed_literal {
                            self.emit_annotation(val, &t.ty);
                        }
                        self.bind_pattern_irrefutable(&t.pat, val)?;
                    } else {
                        self.bind_pattern_irrefutable(&local.pat, val)?;
                    }
                    if is_last {
                        self.emit(Op::LoadUnit { dst });
                    }
                }
                Stmt::Expr(expr, semi) => {
                    if is_last && semi.is_none() {
                        self.compile_into(dst, expr)?;
                    } else {
                        // A statement position method call discards its result,
                        // so the VM can skip building it.
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
        }
        Ok(())
    }

    /// Apply a `let` annotation to an already-computed init value. A numeric
    /// primitive retags through a cast, which only ever acts on a bare
    /// literal's value, an init typed by the real checker already has the
    /// annotated type. Everything else goes through the struct coercion.
    fn emit_annotation(&mut self, reg: Reg, ty: &syn::Type) {
        if numeric_annotation(ty).is_some() {
            let idx = self.add_cast(ty.clone());
            self.emit(Op::Cast {
                dst: reg,
                src: reg,
                ty: idx,
            });
            return;
        }
        self.emit_coerce(reg, ty);
    }

    /// Emit a coercion of `reg` into the annotated type when it names a struct,
    /// `Vec<T>`, or `Option<T>`. Falls back to a no-op path the VM understands.
    pub(super) fn emit_coerce(&mut self, reg: Reg, ty: &syn::Type) {
        // A plain type that is not a user struct or alias, `f64` or `usize`,
        // can never coerce. Skipping the op here keeps annotated lets in hot
        // loops free of runtime type resolution.
        let syn::Type::Path(p) = ty else {
            // Coercion only ever acts on path types.
            return;
        };
        if let Some(seg) = p.path.segments.last()
            && !matches!(seg.arguments, syn::PathArguments::AngleBracketed(_))
            && self
                .ctx
                .resolver
                .resolve_struct_key(self.ctx.module, &p.path)
                .is_none()
        {
            let segs: Vec<String> = p
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            if !matches!(self.resolve_path_res(&segs), Ok(Res::Alias(..))) {
                return;
            }
        }
        let idx = self.add_cast(ty.clone());
        self.emit(Op::Coerce {
            dst: reg,
            src: reg,
            ty: idx,
        });
    }

    // -- expressions -------------------------------------------------------

    /// Compile `expr`, returning the register holding its value. A plain local
    /// returns its own register with no copy.
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
            Expr::Lit(lit) => self.compile_lit(dst, &lit.lit)?,
            Expr::Paren(p) => self.compile_into(dst, &p.expr)?,
            Expr::Group(g) => self.compile_into(dst, &g.expr)?,
            Expr::Reference(r) => self.compile_into(dst, &r.expr)?,
            Expr::Unsafe(u) => self.compile_block(&u.block, dst)?,
            Expr::Block(b) => self.compile_block(&b.block, dst)?,
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
            Expr::Return(r) => {
                let src = match &r.expr {
                    Some(e) => self.compile_expr(e)?,
                    None => {
                        let u = self.alloc();
                        self.emit(Op::LoadUnit { dst: u });
                        u
                    }
                };
                self.emit(Op::Ret { src });
            }
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
                    count: t.elems.len() as u16,
                });
            }
            Expr::Array(a) => {
                let base = self.compile_args(a.elems.iter())?;
                self.emit(Op::MakeVec {
                    dst,
                    base,
                    count: a.elems.len() as u16,
                });
            }
            Expr::Repeat(r) => {
                let val = self.compile_expr(&r.expr)?;
                let count = self.compile_expr(&r.len)?;
                self.emit(Op::MakeArrayRepeat { dst, val, count });
            }
            Expr::Index(idx) => {
                let base = self.compile_expr(&idx.expr)?;
                // A slice like `v[1..]` may leave the end open. Only an index
                // position allows that, a plain range still needs both ends.
                let key = match &*idx.index {
                    Expr::Range(r) => {
                        let key = self.alloc();
                        self.compile_slice_range(key, r)?;
                        key
                    }
                    other => self.compile_expr(other)?,
                };
                self.emit(Op::Index { dst, base, key });
            }
            Expr::Field(f) => {
                let base = self.compile_expr(&f.base)?;
                let member = self.member_of(&f.member);
                self.emit(Op::GetField { dst, base, member });
            }
            Expr::Struct(s) => self.compile_struct_literal(dst, s)?,
            Expr::Range(r) => self.compile_range(dst, r)?,
            Expr::Try(t) => {
                let src = self.compile_expr(&t.expr)?;
                self.emit(Op::Try { dst, src });
            }
            Expr::Cast(c) => {
                let src = self.compile_expr(&c.expr)?;
                let ty = self.add_cast((*c.ty).clone());
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

    pub(super) fn compile_lit(&mut self, dst: Reg, lit: &Lit) -> Result<()> {
        match lit {
            Lit::Int(i) => self.compile_int_lit(dst, i, false, None)?,
            Lit::Bool(b) => self.emit(Op::LoadBool { dst, v: b.value }),
            Lit::Float(f) => self.compile_float_lit(dst, f, false, None)?,
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
                v: b.value() as i64,
            }),
            Lit::ByteStr(bs) => {
                let k = self.add_const(Const::Bytes(Arc::from(bs.value().as_slice())));
                self.emit(Op::LoadConst { dst, k });
            }
            other => bail!("unsupported literal: {other:?}"),
        }
        Ok(())
    }

    /// An integer literal with its real width: from its suffix first, else
    /// from an annotation the caller saw, else untyped. Parses through u128
    /// so a bare literal past i64::MAX, which real Rust types as u64 or
    /// usize, still loads with its full value. The sign of an enclosing
    /// negation comes in as `negated` so `-128i8` and `-9223372036854775808`
    /// type before they could overflow.
    fn compile_int_lit(
        &mut self,
        dst: Reg,
        lit: &syn::LitInt,
        negated: bool,
        annotation: Option<IntWidth>,
    ) -> Result<()> {
        let raw: u128 = lit.base10_parse()?;
        if raw > u128::from(u64::MAX) {
            bail!("integer literal does not fit any supported width");
        }
        let mut value = raw as i128;
        if negated {
            value = -value;
        }
        let width = match lit.suffix() {
            "" | "u128" | "i128" => annotation,
            suffix => Some(
                IntWidth::parse(suffix)
                    .ok_or_else(|| anyhow!("unsupported literal suffix `{suffix}`"))?,
            ),
        };
        let width = width.unwrap_or({
            // Untyped and past i64::MAX can only be u64 or usize in a valid
            // program, and those two share one runtime semantic.
            if value > i128::from(i64::MAX) {
                IntWidth::U64
            } else {
                IntWidth::I64
            }
        });
        match width {
            IntWidth::I64 => self.emit(Op::LoadInt {
                dst,
                v: value as i64,
            }),
            w => self.emit(Op::LoadIntW {
                dst,
                v: w.encode(truncate(value, w)),
                w,
            }),
        }
        Ok(())
    }

    /// A float literal at its real width. An f32 parses from its own digits,
    /// never through f64 rounding.
    fn compile_float_lit(
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

    /// Compile an annotated numeric init directly at the annotated type when
    /// it is a plain literal, possibly negated or parenthesized. False means
    /// the init needs a runtime cast after normal compilation.
    fn compile_numeric_annotated(
        &mut self,
        dst: Reg,
        expr: &Expr,
        target: NumericTy,
    ) -> Result<bool> {
        match expr {
            Expr::Paren(p) => self.compile_numeric_annotated(dst, &p.expr, target),
            Expr::Group(g) => self.compile_numeric_annotated(dst, &g.expr, target),
            Expr::Unary(u) if matches!(u.op, UnOp::Neg(_)) => {
                self.compile_numeric_lit(dst, &u.expr, true, target)
            }
            other => self.compile_numeric_lit(dst, other, false, target),
        }
    }

    fn compile_numeric_lit(
        &mut self,
        dst: Reg,
        expr: &Expr,
        negated: bool,
        target: NumericTy,
    ) -> Result<bool> {
        let Expr::Lit(l) = expr else {
            return Ok(false);
        };
        match (&l.lit, target) {
            (Lit::Int(i), NumericTy::Int(width)) => {
                self.compile_int_lit(dst, i, negated, Some(width))?;
                Ok(true)
            }
            (Lit::Float(f), NumericTy::Float(ty)) => {
                self.compile_float_lit(dst, f, negated, Some(ty))?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub(super) fn compile_path(&mut self, dst: Reg, path: &syn::Path) -> Result<()> {
        if path.segments.len() == 1 {
            let name = path.segments[0].ident.to_string();
            return self.load_name(&name, dst);
        }
        // A multi segment path used as a value, resolved against the module.
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        self.compile_resolved_value(dst, &segs)
    }

    pub(super) fn compile_unary(&mut self, dst: Reg, u: &syn::ExprUnary) -> Result<()> {
        if matches!(u.op, UnOp::Deref(_)) {
            let src = self.compile_expr(&u.expr)?;
            self.emit(Op::Deref { dst, src });
            return Ok(());
        }
        // A negated literal types as one token, so `-128i8` and the i64
        // minimum load directly instead of negating an unrepresentable
        // positive value.
        if matches!(u.op, UnOp::Neg(_))
            && let Expr::Lit(l) = &*u.expr
        {
            match &l.lit {
                Lit::Int(i) => return self.compile_int_lit(dst, i, true, None),
                Lit::Float(f) => return self.compile_float_lit(dst, f, true, None),
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
        // Compound assignment, `a += b`, mutates in place and yields unit.
        if is_assign_op(&b.op) {
            let op = bin_kind(&b.op).ok_or_else(|| anyhow!("unsupported operator {:?}", b.op))?;
            self.compile_compound_assign(&b.left, op, &b.right)?;
            self.emit(Op::LoadUnit { dst });
            return Ok(());
        }
        // Short circuiting logical operators.
        match b.op {
            BinOp::And(_) => {
                self.compile_into(dst, &b.left)?;
                let jmp = self.here();
                self.emit(Op::JumpIfFalse { cond: dst, to: 0 });
                self.compile_into(dst, &b.right)?;
                let end = self.here() as u32;
                self.patch_jump(jmp, end);
                return Ok(());
            }
            BinOp::Or(_) => {
                self.compile_into(dst, &b.left)?;
                let jmp = self.here();
                self.emit(Op::JumpIfTrue { cond: dst, to: 0 });
                self.compile_into(dst, &b.right)?;
                let end = self.here() as u32;
                self.patch_jump(jmp, end);
                return Ok(());
            }
            _ => {}
        }
        let op = bin_kind(&b.op).ok_or_else(|| anyhow!("unsupported operator {:?}", b.op))?;
        let a = self.compile_expr(&b.left)?;
        // A literal immediate is width-safe: when the left side carries a
        // width the general path adopts it, exactly like a bare literal.
        if let Some(imm) = int_literal(&b.right) {
            self.emit(Op::BinImm { dst, a, imm, op });
            return Ok(());
        }
        let c = self.compile_expr(&b.right)?;
        self.emit(Op::Bin { dst, a, b: c, op });
        Ok(())
    }

    // -- statements ----------------------------------------------------------

    /// Compile a branch condition and emit the jump taken when it is false,
    /// returning the jump's index for patching. A plain comparison becomes a
    /// fused compare-and-branch instead of a Bin plus JumpIfFalse pair.
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
            if let Some(imm) = int_literal(&b.right) {
                let at = self.here();
                self.emit(Op::CmpJumpImm { a, imm, op, to: 0 });
                return Ok(at);
            }
            let c = self.compile_expr(&b.right)?;
            let at = self.here();
            self.emit(Op::CmpJump { a, b: c, op, to: 0 });
            return Ok(at);
        }
        let c = self.compile_expr(cond)?;
        let at = self.here();
        self.emit(Op::JumpIfFalse { cond: c, to: 0 });
        Ok(at)
    }

    pub(super) fn compile_range(&mut self, dst: Reg, r: &syn::ExprRange) -> Result<()> {
        let start = match &r.start {
            Some(e) => self.compile_expr(e)?,
            None => {
                let z = self.alloc();
                self.emit(Op::LoadInt { dst: z, v: 0 });
                z
            }
        };
        let end = match &r.end {
            Some(e) => self.compile_expr(e)?,
            None => bail!("open ended ranges are not supported"),
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

    /// A range in index position. An open end becomes an i64::MAX sentinel
    /// that the slicing code reads as "to the end".
    fn compile_slice_range(&mut self, dst: Reg, r: &syn::ExprRange) -> Result<()> {
        if r.end.is_some() {
            return self.compile_range(dst, r);
        }
        let start = match &r.start {
            Some(e) => self.compile_expr(e)?,
            None => {
                let z = self.alloc();
                self.emit(Op::LoadInt { dst: z, v: 0 });
                z
            }
        };
        let end = self.alloc();
        self.emit(Op::LoadInt {
            dst: end,
            v: i64::MAX,
        });
        self.emit(Op::MakeRange {
            dst,
            start,
            end,
            inclusive: false,
        });
        Ok(())
    }

    // -- control flow ------------------------------------------------------

    pub(super) fn compile_if(&mut self, dst: Reg, if_expr: &syn::ExprIf) -> Result<()> {
        // `if let PAT = EXPR { .. }` and let-chains like
        // `if let Some(x) = a && x > 0 && let Ok(y) = b { .. }`. The chain is a
        // left-nested `&&` whose terms may each be a `let` binding or a plain
        // condition. All terms must pass, and earlier bindings are in scope for
        // later terms and the body.
        let terms = flatten_and(&if_expr.cond);
        if terms.iter().any(|t| matches!(t, Expr::Let(_))) {
            self.push_scope();
            let mut else_jumps = Vec::new();
            for term in &terms {
                if let Expr::Let(let_expr) = term {
                    let scrut = self.compile_expr(&let_expr.expr)?;
                    let matched = self.alloc();
                    let pat = self.pattern_info(&let_expr.pat)?;
                    self.emit(Op::TestBind {
                        val: scrut,
                        pat,
                        dst: matched,
                    });
                    else_jumps.push(self.here());
                    self.emit(Op::JumpIfFalse {
                        cond: matched,
                        to: 0,
                    });
                } else {
                    let cond = self.compile_expr(term)?;
                    else_jumps.push(self.here());
                    self.emit(Op::JumpIfFalse { cond, to: 0 });
                }
            }
            self.compile_block_inner(&if_expr.then_branch, dst)?;
            self.pop_scope();
            let jmp_end = self.here();
            self.emit(Op::Jump { to: 0 });
            let else_at = self.here() as u32;
            for j in else_jumps {
                self.patch_jump(j, else_at);
            }
            match &if_expr.else_branch {
                Some((_, e)) => self.compile_into(dst, e)?,
                None => self.emit(Op::LoadUnit { dst }),
            }
            let end = self.here() as u32;
            self.patch_jump(jmp_end, end);
            return Ok(());
        }
        let jmp_else = self.emit_cond_jump(&if_expr.cond)?;
        self.compile_block(&if_expr.then_branch, dst)?;
        let jmp_end = self.here();
        self.emit(Op::Jump { to: 0 });
        let else_at = self.here() as u32;
        self.patch_jump(jmp_else, else_at);
        match &if_expr.else_branch {
            Some((_, e)) => self.compile_into(dst, e)?,
            None => self.emit(Op::LoadUnit { dst }),
        }
        let end = self.here() as u32;
        self.patch_jump(jmp_end, end);
        Ok(())
    }

    pub(super) fn compile_while(&mut self, dst: Reg, w: &syn::ExprWhile) -> Result<()> {
        let head = self.here();
        // `while let PAT = EXPR` support.
        if let Expr::Let(let_expr) = &*w.cond {
            let scrut = self.compile_expr(&let_expr.expr)?;
            self.push_scope();
            let matched = self.alloc();
            let pat = self.pattern_info(&let_expr.pat)?;
            self.emit(Op::TestBind {
                val: scrut,
                pat,
                dst: matched,
            });
            let exit = self.here();
            self.emit(Op::JumpIfFalse {
                cond: matched,
                to: 0,
            });
            self.loops.push(LoopCtx {
                breaks: vec![exit],
                continue_to: head,
                result: dst,
            });
            let body = self.alloc();
            self.compile_block_inner(&w.body, body)?;
            self.pop_scope();
            self.emit(Op::Jump { to: head as u32 });
            let end = self.here() as u32;
            let lc = self.loops.pop().unwrap();
            for b in lc.breaks {
                self.patch_jump(b, end);
            }
            self.emit(Op::LoadUnit { dst });
            return Ok(());
        }
        let exit = self.emit_cond_jump(&w.cond)?;
        self.loops.push(LoopCtx {
            breaks: vec![exit],
            continue_to: head,
            result: dst,
        });
        let body = self.alloc();
        self.compile_block(&w.body, body)?;
        self.emit(Op::Jump { to: head as u32 });
        let end = self.here() as u32;
        let lc = self.loops.pop().unwrap();
        for b in lc.breaks {
            self.patch_jump(b, end);
        }
        self.emit(Op::LoadUnit { dst });
        Ok(())
    }

    pub(super) fn compile_loop(&mut self, dst: Reg, l: &syn::ExprLoop) -> Result<()> {
        self.emit(Op::LoadUnit { dst });
        let head = self.here();
        self.loops.push(LoopCtx {
            breaks: Vec::new(),
            continue_to: head,
            result: dst,
        });
        let body = self.alloc();
        self.compile_block(&l.body, body)?;
        self.emit(Op::Jump { to: head as u32 });
        let end = self.here() as u32;
        let lc = self.loops.pop().unwrap();
        for b in lc.breaks {
            self.patch_jump(b, end);
        }
        Ok(())
    }

    pub(super) fn compile_for(&mut self, dst: Reg, f: &syn::ExprForLoop) -> Result<()> {
        let src = self.compile_expr(&f.expr)?;
        let iter = self.alloc();
        self.emit(Op::IterInit { dst: iter, src });
        let idx = self.alloc();
        self.emit(Op::LoadInt { dst: idx, v: 0 });
        let val = self.alloc();
        let head = self.here();
        let next = self.here();
        self.emit(Op::ForNext {
            iter,
            idx,
            val,
            to: 0,
        });
        self.push_scope();
        self.bind_pattern_irrefutable(&f.pat, val)?;
        self.loops.push(LoopCtx {
            breaks: vec![next],
            continue_to: head,
            result: dst,
        });
        let body = self.alloc();
        self.compile_block_inner(&f.body, body)?;
        self.pop_scope();
        self.emit(Op::Jump { to: head as u32 });
        let end = self.here() as u32;
        let lc = self.loops.pop().unwrap();
        for b in lc.breaks {
            self.patch_jump(b, end);
        }
        self.emit(Op::LoadUnit { dst });
        Ok(())
    }

    pub(super) fn compile_break(&mut self, b: &syn::ExprBreak) -> Result<()> {
        let result = self.loops.last().map(|l| l.result);
        if let Some(result) = result {
            if let Some(e) = &b.expr {
                self.compile_into(result, e)?;
            }
        } else {
            bail!("break outside a loop");
        }
        let jmp = self.here();
        self.emit(Op::Jump { to: 0 });
        self.loops.last_mut().unwrap().breaks.push(jmp);
        Ok(())
    }

    pub(super) fn compile_continue(&mut self) -> Result<()> {
        let to = self
            .loops
            .last()
            .map(|l| l.continue_to)
            .ok_or_else(|| anyhow!("continue outside a loop"))?;
        self.emit(Op::Jump { to: to as u32 });
        Ok(())
    }

    pub(super) fn compile_match(&mut self, dst: Reg, m: &syn::ExprMatch) -> Result<()> {
        let scrut = self.compile_expr(&m.expr)?;
        let mut end_jumps = Vec::new();
        for arm in &m.arms {
            self.push_scope();
            let matched = self.alloc();
            let pat = self.pattern_info(&arm.pat)?;
            self.emit(Op::TestBind {
                val: scrut,
                pat,
                dst: matched,
            });
            let skip = self.here();
            self.emit(Op::JumpIfFalse {
                cond: matched,
                to: 0,
            });
            // Guard.
            let mut guard_skip = None;
            if let Some((_, guard)) = &arm.guard {
                let g = self.compile_expr(guard)?;
                let gs = self.here();
                self.emit(Op::JumpIfFalse { cond: g, to: 0 });
                guard_skip = Some(gs);
            }
            self.compile_into(dst, &arm.body)?;
            let je = self.here();
            self.emit(Op::Jump { to: 0 });
            end_jumps.push(je);
            self.pop_scope();
            let next = self.here() as u32;
            self.patch_jump(skip, next);
            if let Some(gs) = guard_skip {
                self.patch_jump(gs, next);
            }
        }
        // No arm matched, a runtime error mirroring the old behavior.
        let p = self.add_path(vec!["::unreachable_match".to_string()], None);
        self.emit(Op::CallPath {
            dst,
            path: p,
            base: dst,
            argc: 0,
        });
        let end = self.here() as u32;
        for j in end_jumps {
            self.patch_jump(j, end);
        }
        Ok(())
    }

    // -- calls -------------------------------------------------------------
}
