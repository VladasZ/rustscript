//! Blocks, statements and `let`.

use anyhow::{Result, bail};
use syn::spanned::Spanned;
use syn::{Block, Expr, Pat, Stmt};

use crate::interpreter::bytecode::{DISCARD, Op, Reg};
use crate::interpreter::numeric::IntWidth;

use super::walks::{
    annotation_scalar, bare_int_arithmetic, bare_int_rooted, collect_root, from_str_root,
    takes_numeric_hint, unparen,
};

use super::{
    CollectTarget, Compiler, NumericTy, ScalarTy, macro_yields_value, numeric_annotation,
    numeric_target,
};

impl Compiler<'_> {
    pub(super) fn compile_block(&mut self, block: &Block, dst: Reg) -> Result<()> {
        self.push_scope();
        // the written locals of the block end with it, so a later block reusing a name can't read
        // the wrong type
        let typed_locals = self.typed_locals.clone();
        let typed_local_types = self.typed_local_types.clone();
        let res = self.compile_block_inner(block, dst);
        self.typed_locals = typed_locals;
        self.typed_local_types = typed_local_types;
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
                        self.compile_into(dst, expr)?;
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
        // an annotated `let` rooted in `from_str` hands its type to the call, so no coerce op is needed
        let mut offered = false;
        // a nested let runs this again before the outer collect consumes its hint, so the outer
        // hint is restored
        let outer_collect_let = self.collect_let.take();
        if let Pat::Type(t) = &local.pat
            && let Some(init) = &local.init
        {
            if let Some(call) = from_str_root(&init.expr) {
                self.json_let = Some((std::ptr::from_ref(call), self.lower_ir(&t.ty)));
                offered = true;
            } else if let Some(call) = super::struct_lit::bare_default_call(&init.expr) {
                // `let x: T = Default::default()`
                if let Some(ir) = self.default_ir(&t.ty) {
                    self.default_calls.insert(std::ptr::from_ref(call), ir);
                }
            } else if let Some(target) = CollectTarget::of_type(&t.ty)
                && let Some(mc) = collect_root(&init.expr)
            {
                self.collect_let = Some((std::ptr::from_ref(mc), target));
            }
            // `let x: T = ...unwrap_or_default()`
            if let Expr::MethodCall(mc) = unparen(&init.expr)
                && mc.method == "unwrap_or_default"
            {
                if let Some(ty) = ScalarTy::lower(&t.ty) {
                    self.default_let = Some((std::ptr::from_ref(mc), ty));
                }
                self.default_let_ty = Some((std::ptr::from_ref(mc), (*t.ty).clone()));
            }
            // `let x: T = ...sum()` names the width like a turbofish
            if let Expr::MethodCall(mc) = unparen(&init.expr)
                && (mc.method == "sum" || mc.method == "product")
                && mc.turbofish.is_none()
                && let Some(ty) = ScalarTy::lower(&t.ty)
            {
                self.reduce_let = Some((std::ptr::from_ref(mc), ty));
            }
            // `let x: T = v.into()`
            if let Expr::MethodCall(mc) = unparen(&init.expr)
                && mc.method == "into"
                && mc.args.is_empty()
                && let syn::Type::Path(p) = &*t.ty
                && let Some(canon) = self.user_type_key(&p.path)
            {
                self.into_let = Some((std::ptr::from_ref(mc), canon));
            }
        }
        // `let opt: Option<T>` and `let v: Vec<T>` record the type for a later `unwrap_or_default()`
        if let Pat::Type(t) = &local.pat
            && let Pat::Ident(ident) = &*t.pat
        {
            self.typed_local_types
                .insert(ident.ident.to_string(), (*t.ty).clone());
            if let Some(declared) = annotation_scalar(&t.ty) {
                self.typed_locals.insert(ident.ident.to_string(), declared);
            }
        }
        // a numeric annotation types a bare literal at compile time
        let mut typed_literal = false;
        if let Pat::Type(t) = &local.pat
            && let Some(init) = &local.init
        {
            if let Some(target) = numeric_annotation(&t.ty) {
                typed_literal = self.compile_numeric_annotated(val, &init.expr, target)?;
            } else {
                self.offer_literal_hints(&t.ty, &init.expr);
            }
        }
        self.seed_unannotated_hint(local);
        if !typed_literal {
            match &local.init {
                Some(init) => self.compile_into(val, &init.expr)?,
                None => self.emit(Op::LoadUnit { dst: val }),
            }
        }
        // `let sorted = vec!['a', 'b']` states its type through the init, read after the init
        // compiles so its block locals are recorded
        if !matches!(&local.pat, Pat::Type(_))
            && let Pat::Ident(ident) = &local.pat
            && let Some(init) = &local.init
        {
            if let Some(stated) = self.stated_ty(&init.expr) {
                self.typed_locals.insert(ident.ident.to_string(), stated);
            }
            if let Some(ty) = self.written_type(&init.expr) {
                self.typed_local_types.insert(ident.ident.to_string(), ty);
            }
        }
        self.record_tuple_pattern_types(local);
        let consumed = offered && self.json_let.is_none();
        self.json_let = None;
        self.collect_let = outer_collect_let;
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
        Ok(())
    }

    /// A type for each name of a tuple pattern `let`. The annotation wins, otherwise each name
    /// reads its own init element.
    pub(super) fn record_tuple_pattern_types(&mut self, local: &syn::Local) {
        if let Some(init) = &local.init {
            let bare = match &local.pat {
                Pat::Type(t) => &*t.pat,
                other => other,
            };
            if let Pat::Tuple(names) = bare
                && let Expr::Tuple(values) = unparen(&init.expr)
                && names.elems.len() == values.elems.len()
            {
                for (pat, value) in names.elems.iter().zip(&values.elems) {
                    let Pat::Ident(ident) = pat else {
                        continue;
                    };
                    let name = ident.ident.to_string();
                    if let Some(stated) = self.stated_ty(value) {
                        self.typed_locals.insert(name.clone(), stated);
                    }
                    if let Some(ty) = self.written_type(value) {
                        self.typed_local_types.insert(name, ty);
                    }
                }
            }
        }
        if let Pat::Type(t) = &local.pat
            && let Pat::Tuple(names) = &*t.pat
            && let syn::Type::Tuple(types) = &*t.ty
            && names.elems.len() == types.elems.len()
        {
            for (pat, ty) in names.elems.iter().zip(&types.elems) {
                let Pat::Ident(ident) = pat else {
                    continue;
                };
                let name = ident.ident.to_string();
                if let Some(scalar) = annotation_scalar(ty) {
                    self.typed_locals.insert(name.clone(), scalar);
                }
                self.typed_local_types.insert(name, ty.clone());
            }
        }
    }

    pub(super) fn seed_unannotated_hint(&mut self, local: &syn::Local) {
        if !matches!(&local.pat, Pat::Type(_))
            && let Some(init) = &local.init
            && takes_numeric_hint(&init.expr)
        {
            // bare literals are `i32`, otherwise `-(i32::MIN)` widens to i64 and never panics
            let target = match self.stated_ty(&init.expr).as_ref().and_then(numeric_target) {
                Some(stated) => Some(stated),
                None => (bare_int_rooted(&init.expr) && bare_int_arithmetic(&init.expr))
                    .then_some(NumericTy::Int(IntWidth::I32)),
            };
            if let Some(target) = target {
                self.numeric_hints
                    .insert(std::ptr::from_ref(&*init.expr), target);
            }
        }
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
