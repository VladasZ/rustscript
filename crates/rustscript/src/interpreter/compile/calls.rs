//! Calls, closures, assignment, struct literals and patterns.

use std::sync::Arc;

use anyhow::{Result, bail};
use syn::{Expr, Lit, Pat, UnOp};

use crate::interpreter::bytecode::StructShape;
use crate::interpreter::bytecode::{
    BinKind, BuiltinId, CapSource, Const, DISCARD, DefaultIr, EnumVariant, FieldName, Member, Op,
    PLit, PPat, PTag, PatInfo, PathId, PathRef, Reg, ScalarTy, StructLit,
};
use crate::interpreter::enum_def::{EnumDef, builtin_enum, prelude_variant};
use crate::interpreter::numeric::IntWidth;
use crate::interpreter::serde_attrs::serde_rename;
use crate::interpreter::typeir::CastIr;

use super::expr::{annotation_scalar, tail_exprs, takes_numeric_hint, unparen};
use super::place;
use super::written::{TyEnv, element_of, option_payload, turbofish_scalar, written_ty};
use super::{
    CollectTarget, Compiler, FnState, NameLoc, NumericTy, Res, TypeIr, collect_pattern_names,
    first_generic_type, idx16, int_literal, numeric_annotation, numeric_target,
};

impl Compiler<'_> {
    /// The window is reserved first so an argument's own temporaries cannot
    /// break the packing.
    pub(super) fn compile_args<'e>(&mut self, args: impl Iterator<Item = &'e Expr>) -> Result<Reg> {
        let list: Vec<&Expr> = args.collect();
        let base = self.cur().reg_top;
        for _ in 0..list.len() {
            self.alloc();
        }
        for (i, a) in list.iter().enumerate() {
            self.compile_into(base + idx16(i), a)?;
            self.emit_arg_move_out(a);
        }
        Ok(base)
    }

    /// A by value argument is a move. With `Drop` impls the binding register
    /// clears after the copy, so the guard drops where the move sent it.
    fn emit_arg_move_out(&mut self, arg: &Expr) {
        if !self.ctx.has_drop {
            return;
        }
        let Some(name) = place::single_path_name(arg) else {
            return;
        };
        if self.cur().aliases.contains_key(&name) {
            return;
        }
        if let NameLoc::Local(reg) = self.resolve(&name)
            // A borrow parameter forwards a reference, so the caller keeps its
            // handle.
            && !self.cur().borrow_params.contains(&reg)
        {
            self.emit(Op::MoveOut { src: reg });
        }
    }

    /// The `AppList` in `get_json::<AppList>(..)`. An index into
    /// `call_type_args`, or `u32::MAX` when there are none.
    fn record_call_type_args(&mut self, path: &syn::Path) -> u32 {
        let Some(seg) = path.segments.last() else {
            return u32::MAX;
        };
        let syn::PathArguments::AngleBracketed(ab) = &seg.arguments else {
            return u32::MAX;
        };
        let mut types = Vec::new();
        for a in &ab.args {
            if let syn::GenericArgument::Type(t) = a {
                types.push(self.lower_ir(t));
            }
        }
        if types.is_empty() {
            return u32::MAX;
        }
        let table = &mut self.cur().call_type_args;
        table.push(Arc::from(types.into_boxed_slice()));
        u32::try_from(table.len() - 1).expect("type-arg table exceeds u32 indices")
    }

    pub(super) fn compile_call(&mut self, dst: Reg, c: &syn::ExprCall) -> Result<()> {
        let Expr::Path(path_expr) = &*c.func else {
            let callee = self.compile_expr(&c.func)?;
            let base = self.compile_args(c.args.iter())?;
            self.emit_borrow_takes(c.args.iter());
            self.emit(Op::CallValue {
                dst,
                callee,
                base,
                argc: idx16(c.args.len()),
            });
            self.emit_mut_arg_writebacks(c.args.iter(), base)?;
            return Ok(());
        };
        let path = &path_expr.path;
        // `tokio::spawn(async { .. })` lowers to a `Spawn` op with the block
        // as a child chunk.
        if self.ctx.async_mode && is_tokio_spawn(path) {
            match c.args.first() {
                Some(Expr::Async(block)) if c.args.len() == 1 => {
                    return self.compile_spawn(dst, &block.block);
                }
                _ => bail!("tokio::spawn needs an async block in this interpreter"),
            }
        }
        if c.args.is_empty()
            && path
                .segments
                .last()
                .is_some_and(|seg| seg.ident == "default")
        {
            if let Some(ir) = self.default_call_ir(c, path_expr) {
                self.emit_default(dst, ir);
                return Ok(());
            }
            // `<S>::default()` on a type with its own `fn default` is
            // `S::default()`.
            if let Some(qself) = &path_expr.qself
                && let syn::Type::Path(tp) = &*qself.ty
            {
                let mut merged = tp.path.clone();
                merged.segments.extend(path.segments.iter().cloned());
                let coerce = self.call_coerce(c, &merged);
                return self.compile_resolved_call(dst, c, &merged, coerce, 0);
            }
        }
        let coerce = self.call_coerce(c, path);
        let argc = idx16(c.args.len());

        if self.try_compile_closure_call(dst, c, path, argc)? {
            return Ok(());
        }
        self.compile_resolved_call(dst, c, path, coerce, argc)
    }

    /// `<T>::default()` names it in the qualified self, `T::default()` in
    /// the path, a bare `Default::default()` takes it from the context hint.
    fn default_call_ir(
        &mut self,
        c: &syn::ExprCall,
        path_expr: &syn::ExprPath,
    ) -> Option<DefaultIr> {
        if let Some(qself) = &path_expr.qself {
            return self.default_ir(&qself.ty);
        }
        let path = &path_expr.path;
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        if segs.len() == 1 {
            return None;
        }
        if segs[..segs.len() - 1] == ["Default"] {
            return self.default_calls.remove(&std::ptr::from_ref(c));
        }
        // A user `impl Default` or inherent `fn default` wins over the derive.
        let owner = segs[segs.len() - 2].clone();
        let user_defined = self.ctx.impl_methods.iter().any(|(ty, name)| {
            name == "default" && (*ty == owner || super::super::resolver::bare(ty) == owner)
        });
        if user_defined {
            return None;
        }
        let prefix = syn::Path {
            leading_colon: path.leading_colon,
            segments: path.segments.iter().take(segs.len() - 1).cloned().collect(),
        };
        self.default_ir_path_pub(&prefix)
    }

    /// Builtin variants and empty container constructors build in place, so
    /// a `Vec::new()` in a loop skips the path dispatch. A `with_capacity`
    /// argument still runs.
    fn compile_builtin_ctor(
        &mut self,
        dst: Reg,
        c: &syn::ExprCall,
        path: &PathRef,
        argc: u16,
    ) -> Result<bool> {
        if let Some((def, index)) = self.resolve_variant(&path.segs)
            && !def.is_unit(index)
        {
            let base = self.compile_args(c.args.iter())?;
            let info = self.add_enum_variant(EnumVariant {
                def,
                variant: index,
            });
            self.emit(Op::MakeEnum {
                dst,
                info,
                base,
                count: argc,
            });
            return Ok(true);
        }
        let Some(kind) = empty_container(path.id) else {
            return Ok(false);
        };
        let base = self.compile_args(c.args.iter())?;
        match kind {
            EmptyKind::Vec => self.emit(Op::MakeVec {
                dst,
                base,
                count: 0,
            }),
            EmptyKind::Str => {
                let k = self.add_const(Const::Str(Arc::from("")));
                self.emit(Op::LoadConst { dst, k });
            }
            EmptyKind::Map => self.emit(Op::MakeMap { dst, set: false }),
            EmptyKind::Set => self.emit(Op::MakeMap { dst, set: true }),
        }
        Ok(true)
    }

    /// The path resolved half of `compile_call`.
    fn compile_resolved_call(
        &mut self,
        dst: Reg,
        c: &syn::ExprCall,
        path: &syn::Path,
        coerce: Option<TypeIr>,
        argc: u16,
    ) -> Result<()> {
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        // `Self::..` inside an impl names the impl's own type.
        let resolved = match (segs.first().map(String::as_str), self.ctx.impl_type) {
            (Some("Self"), Some(ty)) if segs.len() > 1 => {
                Res::TypeMember(Arc::from(ty), segs[1..].to_vec())
            }
            _ => self.resolve_path_res(&segs)?,
        };
        let path = match resolved {
            // Turbofish type args are recorded so the callee can bind them.
            Res::Fn(idx) => {
                let targ = self.record_call_type_args(path);
                let base = self.compile_args(c.args.iter())?;
                self.emit_borrow_takes(c.args.iter());
                self.emit(Op::CallFn {
                    dst,
                    func: idx,
                    base,
                    argc,
                    targ,
                });
                self.emit_mut_arg_writebacks(c.args.iter(), base)?;
                return Ok(());
            }
            Res::Struct(canon) => PathRef::user(vec![canon.to_string()], coerce),
            Res::TypeMember(canon, rest) => {
                if let Some(variant) = self.enum_variant(&canon, &rest, |fields| {
                    matches!(fields, syn::Fields::Unnamed(fields) if fields.unnamed.len() == argc as usize)
                }) {
                    let base = self.compile_args(c.args.iter())?;
                    let info = self.add_enum_variant(variant);
                    self.emit(Op::MakeEnum {
                        dst,
                        info,
                        base,
                        count: argc,
                    });
                    return Ok(());
                }
                let mut segs = vec![canon.to_string()];
                segs.extend(rest);
                PathRef::user(segs, coerce)
            }
            // `type P = Point; P(..)`.
            Res::Alias(m, target) => {
                let aliased = match &*target {
                    syn::Type::Path(p) => self.ctx.resolver.resolve_struct_key(m, &p.path),
                    _ => None,
                };
                match aliased {
                    Some(canon) => PathRef::user(vec![canon.to_string()], coerce),
                    None => bail!("cannot call `{}`", segs.join("::")),
                }
            }
            Res::Enum(_) | Res::Module | Res::Const(_) => {
                bail!("cannot call `{}`", segs.join("::"))
            }
            Res::External(segs) => {
                let path = self.external_path(segs, coerce);
                match self.compile_external_call(dst, c, path, argc)? {
                    Some(path) => path,
                    None => return Ok(()),
                }
            }
        };
        // `drop(x)` moves `x` out, so its register clears and the callee sees
        // the last holder.
        let cleared = if path.id == PathId::Drop && c.args.len() == 1 {
            place::single_path_name(&c.args[0]).and_then(|n| {
                let n = self.unalias(&n);
                match self.resolve(&n) {
                    NameLoc::Local(reg) => Some(reg),
                    _ => None,
                }
            })
        } else {
            None
        };
        let p = self.add_path(path);
        let base = self.compile_args(c.args.iter())?;
        if let Some(reg) = cleared {
            self.emit(Op::LoadUnit { dst: reg });
        }
        self.emit(Op::CallPath {
            dst,
            path: p,
            base,
            argc,
        });
        Ok(())
    }

    /// Answers the path back when the call still needs the VM.
    fn compile_external_call(
        &mut self,
        dst: Reg,
        c: &syn::ExprCall,
        path: PathRef,
        argc: u16,
    ) -> Result<Option<PathRef>> {
        // Only `Box::new` is a pass through, a box is pure ownership. `Rc`,
        // `Arc`, `RefCell`, `Cell` and `Mutex` build real shared cells.
        if path.id == PathId::BoxNew && c.args.len() == 1 {
            self.compile_into(dst, &c.args[0])?;
            return Ok(None);
        }
        // The mem place functions move values between places, which only the
        // compiler can express.
        if matches!(
            path.id,
            PathId::MemSwap | PathId::MemTake | PathId::MemReplace
        ) && self.compile_mem_intrinsic(dst, path.id, c)?
        {
            return Ok(None);
        }
        // Numeric `T::from(x)` is the `x as T` cast op, `rustc` already proved
        // it lossless.
        if let Some(ir) = numeric_from_cast(path.id, c.args.len()) {
            let src = self.compile_expr(&c.args[0])?;
            let f = self.cur();
            f.casts.push(ir);
            let ty = idx16(f.casts.len() - 1);
            self.emit(Op::Cast { dst, src, ty });
            return Ok(None);
        }
        if self.compile_builtin_ctor(dst, c, &path, argc)? {
            return Ok(None);
        }
        Ok(Some(path))
    }

    /// Its own turbofish, a pending `let` annotation, or the enclosing
    /// signature. See `Compiler::json_let` and `Compiler::json_tails`.
    fn call_coerce(&mut self, c: &syn::ExprCall, path: &syn::Path) -> Option<TypeIr> {
        let coerce = path
            .segments
            .last()
            .and_then(first_generic_type)
            .map(|t| self.lower_ir(t));
        match coerce {
            Some(ty) => Some(ty),
            None => match &self.json_let {
                Some((ptr, ty)) if std::ptr::eq(*ptr, c) => {
                    let ty = ty.clone();
                    self.json_let = None;
                    Some(ty)
                }
                _ => self.json_tails.get(&std::ptr::from_ref(c)).cloned(),
            },
        }
    }

    /// True when the call was emitted here.
    fn try_compile_closure_call(
        &mut self,
        dst: Reg,
        c: &syn::ExprCall,
        path: &syn::Path,
        argc: u16,
    ) -> Result<bool> {
        if path.segments.len() != 1 {
            return Ok(false);
        }
        let name = path.segments[0].ident.to_string();
        let callee = match self.resolve(&name) {
            NameLoc::Local(reg) => Some(reg),
            NameLoc::Cell(cell) => {
                let reg = self.alloc();
                self.emit(Op::LoadCell { dst: reg, cell });
                Some(reg)
            }
            NameLoc::Upvalue(idx) => {
                let reg = self.alloc();
                self.emit(Op::LoadUpvalue { dst: reg, idx });
                Some(reg)
            }
            NameLoc::None => None,
        };
        let Some(callee) = callee else {
            return Ok(false);
        };
        let base = self.compile_args(c.args.iter())?;
        self.emit_borrow_takes(c.args.iter());
        self.emit(Op::CallValue {
            dst,
            callee,
            base,
            argc,
        });
        self.emit_mut_arg_writebacks(c.args.iter(), base)?;
        Ok(true)
    }

    pub(super) fn compile_method(&mut self, dst: Reg, m: &syn::ExprMethodCall) -> Result<()> {
        self.seed_bare_receiver_width(m);
        self.seed_bare_fallback_width(m);
        if m.method == "copy_from_slice" {
            return self.compile_copy_from_slice(dst, m);
        }
        // `x.get(k).copied().unwrap_or(d)` builds and tears down an Option
        // per call, which dominates counting loops.
        if dst != DISCARD
            && m.method == "unwrap_or"
            && m.args.len() == 1
            && let Expr::MethodCall(c) = &*m.receiver
            && (c.method == "copied" || c.method == "cloned")
            && c.args.is_empty()
            && let Expr::MethodCall(g) = &*c.receiver
            && g.method == "get"
            && g.args.len() == 1
        {
            let recv = self.compile_expr(&g.receiver)?;
            let key = self.compile_expr(&g.args[0])?;
            let default = self.compile_expr(&m.args[0])?;
            self.emit(Op::GetOrDefault {
                dst,
                recv,
                key,
                default,
            });
            return Ok(());
        }
        // An `unwrap_or_default` unwrapped again must have produced an
        // `Option`, so its default is `None`.
        let outer_option_hint = self.option_result.take();
        if m.method == "unwrap_or_default"
            && let Expr::MethodCall(inner) = &*m.receiver
            && inner.method == "unwrap_or_default"
        {
            self.option_result = Some(std::ptr::from_ref(inner));
        }
        // `v.into()` under `let x: T` is `T::from(v)`, without one it is
        // identity.
        if m.method == "into"
            && m.args.is_empty()
            && let Some((ptr, canon)) = &self.into_let
            && std::ptr::eq(*ptr, m)
        {
            let canon = canon.clone();
            self.into_let = None;
            let path = PathRef::user(vec![canon.to_string(), "from".to_string()], None);
            let p = self.add_path(path);
            let base = self.compile_args(std::iter::once(&*m.receiver))?;
            self.emit(Op::CallPath {
                dst,
                path: p,
                base,
                argc: 1,
            });
            return Ok(());
        }
        let method_text = m.method.to_string();
        // `Sum<T> for T` types a `map` closure's body as `T`, so its literals
        // adopt the reduction's width.
        if matches!(method_text.as_str(), "sum" | "product")
            && let Some(target) = self.reduce_target(m)
        {
            self.seed_reduce_closure(&m.receiver, target);
        }
        let mutating = (BuiltinId::resolve(&method_text).mutates()
            || self.ctx.mut_methods.contains(&method_text))
            // `rotate_left` mutates a slice but returns a value on an integer.
            // Writing back over an integer receiver undid the assignment.
            && !(matches!(method_text.as_str(), "rotate_left" | "rotate_right")
                && matches!(self.stated_ty(&m.receiver), Some(ScalarTy::Int(_))));
        let (recv, receiver_place) = if mutating {
            let p = self.compile_mut_receiver(&m.receiver)?;
            (p.reg, Some(p))
        } else {
            (self.compile_expr(&m.receiver)?, None)
        };
        let place = mutating && place::is_place_expr(&m.receiver);
        self.option_result = outer_option_hint;
        // A `fold` closure's accumulator is the init's type and the item is the
        // element, so a default built inside the body knows its type.
        let folded = self.bind_fold_params(m);
        let base = self.compile_args(m.args.iter())?;
        for (name, previous) in folded {
            match previous {
                Some(ty) => self.typed_local_types.insert(name, ty),
                None => self.typed_local_types.remove(&name),
            };
        }
        let (method, scalar) = self.method_name_and_scalar(m);
        let default = if method == "unwrap_or_default" {
            self.default_for_unwrap(m)
        } else {
            None
        };
        let name = self.add_name_full(method, scalar, default, place);
        // Restamp with the method's own line, the one `rustc` names for a
        // multiline chain.
        self.set_line(m.method.span());
        self.emit(Op::Method {
            dst,
            recv,
            name,
            base,
            argc: idx16(m.args.len()),
        });
        if let Some(p) = &receiver_place {
            self.emit_place_writeback(p);
        }
        // `read_line` and friends write into the arg window copy, so move the
        // result back into the variable.
        self.emit_mut_arg_writebacks(m.args.iter(), base)?;
        Ok(())
    }

    /// `v[a..b].copy_from_slice(src)` must write through to `v`, and a range
    /// index builds a copy. So the call compiles against the base vec with
    /// the bounds as leading arguments.
    fn compile_copy_from_slice(&mut self, dst: Reg, m: &syn::ExprMethodCall) -> Result<()> {
        let Expr::Index(ix) = &*m.receiver else {
            bail!("copy_from_slice is only supported on a `v[a..b]` receiver");
        };
        let Expr::Range(r) = &*ix.index else {
            bail!("copy_from_slice is only supported on a `v[a..b]` receiver");
        };
        let Some(src) = m.args.first() else {
            bail!("copy_from_slice takes the source slice");
        };
        let recv = self.compile_expr(&ix.expr)?;
        let base = self.cur().reg_top;
        for _ in 0..3 {
            self.alloc();
        }
        match &r.start {
            Some(e) => self.compile_into(base, e)?,
            None => self.emit(Op::LoadInt { dst: base, v: 0 }),
        }
        match &r.end {
            Some(e) => {
                self.compile_into(base + 1, e)?;
                if matches!(r.limits, syn::RangeLimits::Closed(_)) {
                    self.emit(Op::BinImm {
                        dst: base + 1,
                        a: base + 1,
                        imm: 1,
                        op: BinKind::Add,
                    });
                }
            }
            None => self.emit(Op::LoadInt {
                dst: base + 1,
                v: i64::MAX,
            }),
        }
        self.compile_into(base + 2, src)?;
        let name = self.add_name("copy_from_slice".to_string());
        self.set_line(m.method.span());
        self.emit(Op::Method {
            dst,
            recv,
            name,
            base,
            argc: 3,
        });
        Ok(())
    }

    /// `collect` into a String renames to `collect_string` from a turbofish,
    /// a pending `let s: String`, or a `-> String` signature. See
    /// `Compiler::string_let` and `Compiler::string_tails`.
    fn method_name_and_scalar(&mut self, m: &syn::ExprMethodCall) -> (String, Option<ScalarTy>) {
        let mut method = m.method.to_string();
        if method == "collect" {
            let from_turbofish = m.turbofish.as_ref().and_then(turbofish_collect_target);
            let from_let = match self.collect_let {
                Some((ptr, target)) if std::ptr::eq(ptr, m) => Some(target),
                _ => None,
            };
            let from_tail = self.collect_tails.get(&std::ptr::from_ref(m)).copied();
            if let Some(target) = from_turbofish.or(from_let).or(from_tail) {
                // Cleared only when this call consumed the hint. A nested
                // turbofish collect once cleared it and the outer collect fell
                // back to a vec of pairs.
                if from_let.is_some() {
                    self.collect_let = None;
                }
                method = target.method_name().to_string();
            }
        }
        // The turbofish rides on the name for the methods that need it.
        let mut scalar = turbofish_scalar(m.turbofish.as_ref());
        // `unwrap_or_default` takes its type from the receiver's payload, as
        // `None::<u64>` or `then_some(1u8)` state it.
        if scalar.is_none() && m.method == "unwrap_or_default" {
            let env = TyEnv::new(&self.typed_locals, self.ctx.fn_returns);
            scalar = option_payload(&m.receiver, &env);
        }
        // An empty vec has no shape to dispatch a script method on, so the
        // written type rides along.
        if scalar.is_none() && self.ctx.method_atoms.contains_key(&method) {
            scalar = self.stated_ty(&m.receiver);
        }
        // `concat` of nothing cannot tell nested vecs from strings.
        if scalar.is_none() && method == "concat" {
            let env = TyEnv::new(&self.typed_locals, self.ctx.fn_returns);
            scalar = element_of(&m.receiver, &env);
        }
        // `let x: T = ...sum()` names the width of the outermost reduction.
        if scalar.is_none()
            && (method == "sum" || method == "product")
            && let Some((ptr, ty)) = &self.reduce_let
            && std::ptr::eq(*ptr, m)
        {
            scalar = Some(ty.clone());
            self.reduce_let = None;
        }
        // `let x: T = ...unwrap_or_default()` names the outermost payload.
        if scalar.is_none()
            && let Some((ptr, ty)) = &self.default_let
            && std::ptr::eq(*ptr, m)
        {
            scalar = Some(ty.clone());
            self.default_let = None;
        }
        // A `-> T` signature names a bare reduction or default handed back.
        if scalar.is_none()
            && matches!(method.as_str(), "sum" | "product" | "unwrap_or_default")
            && let Some(ty) = self.return_tails.get(&std::ptr::from_ref(m))
        {
            scalar = ScalarTy::lower(ty);
        }
        // Failing all that, a result unwrapped again is an Option.
        if matches!(self.option_result, Some(ptr) if std::ptr::eq(ptr, m)) {
            self.option_result = None;
            scalar = scalar.or(Some(ScalarTy::Opt(Box::new(ScalarTy::Other))));
        }
        (method, scalar)
    }

    /// Read without consuming the hint.
    fn reduce_target(&self, m: &syn::ExprMethodCall) -> Option<NumericTy> {
        let scalar = turbofish_scalar(m.turbofish.as_ref())
            .or_else(|| match &self.reduce_let {
                Some((ptr, ty)) if std::ptr::eq(*ptr, m) => Some(ty.clone()),
                _ => None,
            })
            .or_else(|| {
                self.return_tails
                    .get(&std::ptr::from_ref(m))
                    .and_then(ScalarTy::lower)
            })?;
        numeric_target(&scalar)
    }

    /// A receiver of only unsuffixed literals is `i32`. Its width picks the
    /// method and the `From` impl, so it is settled first.
    fn seed_bare_receiver_width(&mut self, m: &syn::ExprMethodCall) {
        if self
            .numeric_hints
            .contains_key(&std::ptr::from_ref(&*m.receiver))
            || !super::expr::bare_int_rooted(&m.receiver)
        {
            return;
        }
        self.numeric_hints.insert(
            std::ptr::from_ref(&*m.receiver),
            NumericTy::Int(IntWidth::I32),
        );
    }

    /// `unwrap_or(v)` gives the fallback the payload's type. Without one it
    /// is `i32`.
    fn seed_bare_fallback_width(&mut self, m: &syn::ExprMethodCall) {
        if m.method != "unwrap_or" {
            return;
        }
        let Some(arg) = m.args.first() else {
            return;
        };
        if self.numeric_hints.contains_key(&std::ptr::from_ref(arg))
            || !super::expr::bare_int_rooted(arg)
        {
            return;
        }
        let target = self
            .stated_ty(&m.receiver)
            .and_then(|ty| ty.payload().cloned())
            .as_ref()
            .and_then(numeric_target)
            .unwrap_or(NumericTy::Int(IntWidth::I32));
        self.numeric_hints.insert(std::ptr::from_ref(arg), target);
    }

    /// Returns what each name held before so the caller can put it back.
    fn bind_fold_params(&mut self, m: &syn::ExprMethodCall) -> Vec<(String, Option<syn::Type>)> {
        if m.method != "fold" || m.args.len() != 2 {
            return Vec::new();
        }
        let Some(Expr::Closure(closure)) = m.args.get(1) else {
            return Vec::new();
        };
        let names: Vec<String> = closure
            .inputs
            .iter()
            .map(|input| match input {
                Pat::Ident(id) => Some(id.ident.to_string()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default();
        if names.len() != 2 {
            return Vec::new();
        }
        // The body produces the next accumulator, so it carries the init's
        // width.
        if let Some(target) = m
            .args
            .first()
            .and_then(|init| self.stated_ty(init))
            .as_ref()
            .and_then(numeric_target)
        {
            let mut tails = Vec::new();
            tail_exprs(&closure.body, &mut tails);
            for tail in tails.into_iter().filter(|tail| takes_numeric_hint(tail)) {
                self.numeric_hints.insert(std::ptr::from_ref(tail), target);
            }
        }
        let acc = m.args.first().and_then(|init| self.written_type(init));
        let item = self
            .written_type(&m.receiver)
            .and_then(|recv| super::written_type::sequence_element(&recv))
            .or_else(|| acc.clone());
        let mut saved = Vec::new();
        for (name, ty) in names.into_iter().zip([acc, item]) {
            let Some(ty) = ty else {
                continue;
            };
            let previous = self.typed_local_types.insert(name.clone(), ty);
            saved.push((name, previous));
        }
        saved
    }

    fn seed_reduce_closure(&mut self, expr: &Expr, target: NumericTy) {
        let mut current = unparen(expr);
        loop {
            let Expr::MethodCall(mc) = current else {
                return;
            };
            match mc.method.to_string().as_str() {
                "map" => {
                    if let Some(Expr::Closure(closure)) = mc.args.first() {
                        let mut tails = Vec::new();
                        tail_exprs(&closure.body, &mut tails);
                        for tail in tails.into_iter().filter(|tail| takes_numeric_hint(tail)) {
                            self.numeric_hints.insert(std::ptr::from_ref(tail), target);
                        }
                    }
                    return;
                }
                "iter" | "into_iter" | "copied" | "cloned" | "rev" | "filter" | "take" | "skip"
                | "take_while" | "skip_while" | "peekable" | "by_ref" => {
                    current = unparen(&mc.receiver);
                }
                _ => return,
            }
        }
    }

    /// From the `let` annotation or the receiver chain.
    fn default_for_unwrap(&mut self, m: &syn::ExprMethodCall) -> Option<DefaultIr> {
        if let Some((ptr, ty)) = &self.default_let_ty
            && std::ptr::eq(*ptr, m)
        {
            let ty = ty.clone();
            self.default_let_ty = None;
            return self.default_ir(&ty);
        }
        if let Some(ty) = self.return_tails.get(&std::ptr::from_ref(m))
            && let Some(ir) = self.default_ir(&ty.clone())
        {
            return Some(ir);
        }
        let recv_ty = self.written_type(&m.receiver)?;
        let payload = super::written_type::payload_of(&recv_ty)?;
        self.default_ir(&payload)
    }

    /// Lets `compile_let` record an unannotated `let sorted = vec!['a', 'b']`.
    pub(super) fn stated_ty(&self, expr: &Expr) -> Option<ScalarTy> {
        let env = TyEnv::new(&self.typed_locals, self.ctx.fn_returns);
        written_ty(expr, &env)
    }

    /// The callee worked on the arg window copy and the VM hands it back on
    /// return. Only for calls whose window survives, a `CallPath` consumes
    /// its args.
    fn emit_mut_arg_writebacks<'e>(
        &mut self,
        args: impl Iterator<Item = &'e Expr>,
        base: Reg,
    ) -> Result<()> {
        for (i, arg) in args.enumerate() {
            if let Expr::Reference(r) = arg
                && r.mutability.is_some()
                && let Expr::Path(p) = &*r.expr
                && p.path.segments.len() == 1
                && p.qself.is_none()
            {
                let name = p.path.segments[0].ident.to_string();
                let location = self.resolve_for_write(&name);
                self.emit_name_store(location, base + idx16(i), &name)?;
                continue;
            }
            // The window slot clears after the move, a stale copy would
            // inflate `Rc::strong_count`.
            if let Some(reg) = self.borrowed_local(arg) {
                self.emit(Op::Move {
                    dst: reg,
                    src: base + idx16(i),
                });
                self.emit(Op::LoadUnit {
                    dst: base + idx16(i),
                });
            }
        }
        Ok(())
    }

    /// A mutable local lives in a cell and a `&mut` alias points elsewhere,
    /// so both stay out.
    fn borrowed_local(&mut self, arg: &Expr) -> Option<Reg> {
        let (name, forwarded) = match arg {
            Expr::Reference(r) => (place::single_path_name(&r.expr)?, false),
            other => (place::single_path_name(other)?, true),
        };
        if self.cur().aliases.contains_key(&name) {
            return None;
        }
        let NameLoc::Local(reg) = self.resolve(&name) else {
            return None;
        };
        if forwarded && !self.cur().borrow_params.contains(&reg) {
            return None;
        }
        Some(reg)
    }

    /// The callee then holds the only live handle, so `Rc::strong_count`
    /// reads the same at any depth. The writebacks restore the registers.
    fn emit_borrow_takes<'e>(&mut self, args: impl Iterator<Item = &'e Expr>) {
        let regs: Vec<Reg> = args.filter_map(|arg| self.borrowed_local(arg)).collect();
        for reg in regs {
            self.emit(Op::LoadUnit { dst: reg });
        }
    }

    /// Captures work like a closure's.
    fn compile_spawn(&mut self, dst: Reg, block: &syn::Block) -> Result<()> {
        self.frames.push(FnState::new("<task>".to_string()));
        self.cur().num_params = 0;
        let ret = self.alloc();
        self.compile_block(block, ret)?;
        self.emit(Op::Ret { src: ret });
        let child = self.frames.pop().unwrap();
        let caps: Vec<CapSource> = child.upvalues.iter().map(|(_, s)| *s).collect();
        let mut chunk = child.into_chunk(self.ctx.file.clone())?;
        chunk.module = idx16(self.ctx.module);
        let parent = self.cur();
        let child_idx = idx16(parent.children.len());
        parent.children.push(Arc::new(chunk));
        parent.child_caps.push(caps);
        self.emit(Op::Spawn {
            dst,
            child: child_idx,
        });
        Ok(())
    }

    pub(super) fn compile_closure(&mut self, dst: Reg, c: &syn::ExprClosure) -> Result<()> {
        self.frames.push(FnState::new("<closure>".to_string()));
        let params: Vec<&Pat> = c.inputs.iter().collect();
        self.cur().num_params = params.len();
        for p in &params {
            let reg = self.alloc();
            // Recorded like an annotated let, so a default built from the
            // param knows its type.
            if let Pat::Type(t) = p
                && let Pat::Ident(id) = &*t.pat
                && let Some(declared) = annotation_scalar(&t.ty)
            {
                self.typed_locals.insert(id.ident.to_string(), declared);
            }
            // A reference param shares the caller's storage, so it never
            // splits.
            if let Pat::Type(t) = p
                && matches!(&*t.ty, syn::Type::Reference(_))
            {
                self.cur().borrow_params.insert(reg);
            }
            match p {
                Pat::Ident(id) if id.subpat.is_none() => self.define(&id.ident.to_string(), reg),
                _ => self.bind_pattern_irrefutable(p, reg)?,
            }
            // A numeric annotation retags the value like a fn param.
            if let Pat::Type(t) = p
                && numeric_annotation(&t.ty).is_some()
            {
                let idx = self.add_cast(&t.ty);
                self.emit(Op::Cast {
                    dst: reg,
                    src: reg,
                    ty: idx,
                });
            }
        }
        if let syn::ReturnType::Type(_, ty) = &c.output
            && numeric_annotation(ty).is_some()
        {
            let idx = self.add_cast(ty);
            self.cur().ret_cast = Some(idx);
        }
        let ret = self.alloc();
        self.compile_into(ret, &c.body)?;
        if let Some(idx) = self.cur().ret_cast {
            self.emit(Op::Cast {
                dst: ret,
                src: ret,
                ty: idx,
            });
        }
        self.emit(Op::Ret { src: ret });
        let child = self.frames.pop().unwrap();
        let caps: Vec<CapSource> = child.upvalues.iter().map(|(_, s)| *s).collect();
        let mut chunk = child.into_chunk(self.ctx.file.clone())?;
        chunk.module = idx16(self.ctx.module);
        chunk.moves = c.capture.is_some();
        let chunk = Arc::new(chunk);
        let parent = self.cur();
        let child_idx = idx16(parent.children.len());
        parent.children.push(chunk);
        parent.child_caps.push(caps);
        self.emit(Op::MakeClosure {
            dst,
            child: child_idx,
        });
        Ok(())
    }

    // -- assignment --------------------------------------------------------

    /// `*seq` for a `seq: &mut usize` parameter. A cell promoted or captured
    /// name keeps the strict op.
    fn deref_param_reg(&self, expr: &Expr) -> Option<Reg> {
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

    /// A numeric local types a bare literal here like an annotated `let`, or
    /// a reassigned `i32` prints 64 digits under `{:b}`.
    fn compile_stored_value(&mut self, name: &str, value: &Expr) -> Result<Reg> {
        let Some(target) = self
            .typed_local_types
            .get(name)
            .and_then(numeric_annotation)
        else {
            return self.compile_expr(value);
        };
        let dst = self.alloc();
        if !self.compile_numeric_annotated(dst, value, target)? {
            self.compile_into(dst, value)?;
        }
        Ok(dst)
    }

    pub(super) fn compile_assign(&mut self, target: &Expr, value: &Expr) -> Result<()> {
        match target {
            Expr::Path(p) if p.path.segments.len() == 1 => {
                let name = p.path.segments[0].ident.to_string();
                let location = self.resolve_for_write(&name);
                let value = self.compile_stored_value(&name, value)?;
                self.emit_name_store(location, value, &name)?;
            }
            Expr::Index(idx) => {
                let val = self.compile_expr(value)?;
                // The base splits from sharing before the write.
                let base = self.compile_place_base(&idx.expr)?;
                let key = self.compile_expr(&idx.index)?;
                self.emit(Op::SetIndex { base, key, val });
            }
            Expr::Field(f) => {
                let val = self.compile_expr(value)?;
                let base = self.compile_place_base(&f.base)?;
                let member = self.member_of(&f.member);
                self.emit(Op::SetField { base, member, val });
            }
            Expr::Unary(u) if matches!(u.op, UnOp::Deref(_)) => {
                // `*r = v` on a `&mut variable` alias writes the variable, which
                // may live in an enclosing frame.
                if let Some(name) = place::single_path_name(&u.expr) {
                    let target = match self.unalias(&name) {
                        same if same == name => self.enclosing_alias_target(&name),
                        target => Some(target),
                    };
                    if let Some(target) = target {
                        let location = self.resolve_for_write(&target);
                        let val = self.compile_stored_value(&target, value)?;
                        self.emit_name_store(location, val, &target)?;
                        return Ok(());
                    }
                }
                let val = self.compile_expr(value)?;
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

    /// The right operand evaluates before the place, so its panic fires
    /// first.
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
                if let Some(imm) = int_literal(rhs) {
                    let current = self.load_name_location(location, &name)?;
                    let result = self.alloc();
                    self.emit(Op::BinImm {
                        dst: result,
                        a: current,
                        imm,
                        op,
                    });
                    self.emit_name_store(location, result, &name)?;
                } else {
                    let b = self.compile_expr(rhs)?;
                    let current = self.load_name_location(location, &name)?;
                    let result = self.alloc();
                    self.emit(Op::Bin {
                        dst: result,
                        a: current,
                        b,
                        op,
                    });
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
                self.emit(Op::Bin {
                    dst: res,
                    a: cur,
                    b,
                    op,
                });
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
                self.emit(Op::Bin {
                    dst: res,
                    a: cur,
                    b,
                    op,
                });
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
    fn compile_compound_deref_assign(
        &mut self,
        u: &syn::ExprUnary,
        op: BinKind,
        rhs: &Expr,
    ) -> Result<()> {
        // A `&mut variable` alias reads and writes the variable itself.
        if let Some(name) = place::single_path_name(&u.expr) {
            let target = match self.unalias(&name) {
                // A captured alias lives in an enclosing frame.
                same if same == name => self.enclosing_alias_target(&name),
                target => Some(target),
            };
            if let Some(target) = target {
                let b = self.compile_expr(rhs)?;
                let location = self.resolve_for_write(&target);
                let current = self.load_name_location(location, &target)?;
                let result = self.alloc();
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
        let Some(target) = param else {
            // The fused op holds the lock across the read-modify-write, so
            // concurrent tasks cannot lose updates.
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

    fn load_name_location(&mut self, location: NameLoc, name: &str) -> Result<Reg> {
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

    fn emit_name_store(&mut self, location: NameLoc, src: Reg, name: &str) -> Result<()> {
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

    pub(super) fn compile_struct_literal(&mut self, dst: Reg, s: &syn::ExprStruct) -> Result<()> {
        // A user struct resolves to its canonical name, anything else keeps
        // the last segment.
        let self_type = (s.path.segments.len() == 1 && s.path.segments[0].ident == "Self")
            .then_some(self.ctx.impl_type)
            .flatten();
        let resolved = self_type.map(Arc::<str>::from).or_else(|| {
            self.ctx
                .resolver
                .resolve_struct_key(self.ctx.module, &s.path)
        });
        let (name, def) = if let Some(canon) = resolved {
            let def = self.ctx.resolver.structs.get(&canon).map(|d| d.ast.clone());
            (canon.to_string(), def)
        } else {
            let bare = s
                .path
                .segments
                .last()
                .map(|seg| seg.ident.to_string())
                .unwrap_or_default();
            (bare, None)
        };
        let mut written: Vec<(String, &Expr)> = Vec::new();
        for f in &s.fields {
            let key = match &f.member {
                syn::Member::Named(n) => n.to_string(),
                syn::Member::Unnamed(i) => i.index.to_string(),
            };
            written.push((key, &f.expr));
        }
        // With a `..rest` the shape lists every declared field, without one
        // only the written ones, since the literal must have written all.
        let has_rest = s.rest.is_some();
        let (order, renames) = literal_field_order(def.as_deref(), &written, has_rest);
        // Reserve the window first so field temporaries do not break the
        // packing.
        let slots = order.len() + usize::from(has_rest);
        let base = self.cur().reg_top;
        for _ in 0..slots {
            self.alloc();
        }
        for (i, fname) in order.iter().enumerate() {
            let dstf = base + idx16(i);
            match written.iter().find(|(k, _)| k == fname) {
                Some((_, e)) => {
                    self.field_default_hint(e, def.as_deref(), fname);
                    self.compile_into(dstf, e)?;
                }
                None => self.emit(Op::LoadUnit { dst: dstf }),
            }
        }
        if let Some(rest) = &s.rest {
            self.rest_default_hint(rest, self_type, &s.path);
            self.compile_into(base + idx16(order.len()), rest)?;
        }
        let filled: Vec<bool> = order
            .iter()
            .map(|k| written.iter().any(|(w, _)| w == k))
            .collect();
        let info = {
            let fields: Vec<Arc<str>> = order.into_iter().map(Into::into).collect();
            let known = self
                .shapes
                .iter()
                .find(|s| *s.name == name && s.fields == fields && s.renames == renames);
            let shape = if let Some(shared) = known {
                shared.clone()
            } else {
                let type_id = self.ctx.resolver.type_id_of(&name);
                let built = StructShape::typed(name, type_id, fields, renames);
                self.shapes.push(built.clone());
                built
            };
            let f = self.cur();
            f.struct_lits.push(StructLit {
                shape,
                has_rest,
                filled: filled.into(),
            });
            idx16(f.struct_lits.len() - 1)
        };
        self.emit(Op::MakeStruct { dst, info, base });
        Ok(())
    }

    // -- patterns ----------------------------------------------------------

    pub(super) fn pattern_info(&mut self, pat: &Pat) -> Result<u16> {
        let mut names = Vec::new();
        collect_pattern_names(pat, &mut names);
        let mut binds = Vec::new();
        for n in names {
            let reg = self.alloc();
            self.define(&n, reg);
            binds.push((n, reg));
        }
        let lowered = self.lower_pattern(pat);
        let f = self.cur();
        f.pats.push(PatInfo {
            pat: lowered,
            binds,
        });
        Ok(u16::try_from(f.pats.len() - 1)?)
    }

    /// User enums first, builtin tables second. An unresolved path keeps its
    /// last segment and the runtime test falls back to the name.
    fn variant_tag(&self, path: &syn::Path) -> PTag {
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        self.variant_tag_of(&segs)
    }

    fn variant_tag_of(&self, segs: &[String]) -> PTag {
        PTag {
            name: segs.last().map(|s| Arc::from(s.as_str())),
            variant: self.resolve_variant(segs),
        }
    }

    pub(super) fn resolve_variant(&self, segs: &[String]) -> Option<(Arc<EnumDef>, u16)> {
        if let Ok(Res::TypeMember(canon, rest)) = self.resolve_path_res(segs)
            && let [variant] = rest.as_slice()
            && let Some(def) = self.ctx.resolver.enum_defs.get(&canon)
            && let Some(index) = def.variant_index(variant)
        {
            return Some((def.clone(), index));
        }
        match segs {
            [single] => prelude_variant(single).map(|(def, index)| (def.clone(), index)),
            [.., enum_name, variant] => {
                let def = builtin_enum(enum_name)?;
                Some((def.clone(), def.variant_index(variant)?))
            }
            [] => None,
        }
    }

    fn lower_pattern(&self, pattern: &Pat) -> PPat {
        match pattern {
            Pat::Wild(_) => PPat::Wild,
            Pat::Rest(_) => PPat::Rest,
            Pat::Ident(ident) if is_unit_variant_ident(ident) => PPat::Path {
                tag: self.variant_tag_of(&[ident.ident.to_string()]),
            },
            Pat::Ident(ident) => PPat::Ident {
                name: ident.ident.to_string(),
                sub: ident
                    .subpat
                    .as_ref()
                    .map(|subpattern| Box::new(self.lower_pattern(&subpattern.1))),
            },
            Pat::Lit(literal) => lower_literal(&literal.lit),
            Pat::Paren(paren) => self.lower_pattern(&paren.pat),
            Pat::Reference(reference) => self.lower_pattern(&reference.pat),
            Pat::Type(typed) => self.lower_pattern(&typed.pat),
            Pat::Tuple(tuple) => {
                PPat::Tuple(tuple.elems.iter().map(|p| self.lower_pattern(p)).collect())
            }
            Pat::TupleStruct(tuple) => PPat::TupleStruct {
                tag: self.variant_tag(&tuple.path),
                elems: tuple.elems.iter().map(|p| self.lower_pattern(p)).collect(),
            },
            Pat::Path(path) => PPat::Path {
                tag: self.variant_tag(&path.path),
            },
            Pat::Struct(structure) => PPat::Struct {
                name: structure
                    .path
                    .segments
                    .last()
                    .map(|segment| segment.ident.to_string()),
                fields: structure
                    .fields
                    .iter()
                    .map(|field| {
                        let name = match &field.member {
                            syn::Member::Named(name) => name.to_string(),
                            syn::Member::Unnamed(index) => index.index.to_string(),
                        };
                        (name, self.lower_pattern(&field.pat))
                    })
                    .collect(),
            },
            Pat::Or(or) => PPat::Or(or.cases.iter().map(|p| self.lower_pattern(p)).collect()),
            Pat::Slice(slice) => {
                PPat::Slice(slice.elems.iter().map(|p| self.lower_pattern(p)).collect())
            }
            Pat::Range(range) => lower_range(range),
            _ => PPat::Unsupported,
        }
    }

    pub(super) fn bind_pattern_irrefutable(&mut self, pat: &Pat, reg: Reg) -> Result<()> {
        match pat {
            Pat::Ident(id) if id.subpat.is_none() => {
                self.define(&id.ident.to_string(), reg);
                Ok(())
            }
            Pat::Wild(_) => Ok(()),
            Pat::Type(t) => self.bind_pattern_irrefutable(&t.pat, reg),
            Pat::Paren(p) => self.bind_pattern_irrefutable(&p.pat, reg),
            Pat::Reference(r) => self.bind_pattern_irrefutable(&r.pat, reg),
            _ => {
                let matched = self.alloc();
                let pidx = self.pattern_info(pat)?;
                self.emit(Op::TestBind {
                    val: reg,
                    pat: pidx,
                    dst: matched,
                });
                Ok(())
            }
        }
    }

    // -- macros ------------------------------------------------------------
}

/// The cast target of a numeric `T::from(x)` call.
fn numeric_from_cast(id: PathId, argc: usize) -> Option<CastIr> {
    if argc != 1 {
        return None;
    }
    match id {
        PathId::F64From => Some(CastIr::F64),
        PathId::F32From => Some(CastIr::F32),
        PathId::I8From
        | PathId::I16From
        | PathId::I32From
        | PathId::I64From
        | PathId::I128From
        | PathId::IsizeFrom
        | PathId::U8From
        | PathId::U16From
        | PathId::U32From
        | PathId::U64From
        | PathId::U128From
        | PathId::UsizeFrom => IntWidth::parse(id.namespace()).map(CastIr::Int),
        _ => None,
    }
}

// Real Rust tells a unit variant from a binding by name resolution, which we
// do not have. So an uppercase ident with no `ref`, `mut` or subpattern is a
// variant like `None`. Without this a bare `None` arm matched a `Some`.
pub(super) fn is_unit_variant_ident(id: &syn::PatIdent) -> bool {
    id.by_ref.is_none()
        && id.mutability.is_none()
        && id.subpat.is_none()
        && id
            .ident
            .to_string()
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase())
}

fn lower_range(range: &syn::PatRange) -> PPat {
    // Outer None is an unsupported literal, inner None is unbounded.
    let endpoint = |e: &Option<Box<Expr>>| match e {
        Some(e) => endpoint_lit(e).map(Some),
        None => Some(None),
    };
    let (Some(lo), Some(hi)) = (endpoint(&range.start), endpoint(&range.end)) else {
        return PPat::Unsupported;
    };
    PPat::Range {
        lo,
        hi,
        inclusive: matches!(range.limits, syn::RangeLimits::Closed(_)),
    }
}

/// Including a negated number, seen through parens.
fn endpoint_lit(e: &Expr) -> Option<PLit> {
    match e {
        Expr::Lit(l) => match &l.lit {
            Lit::Int(value) => value.base10_parse().ok().map(PLit::Int),
            Lit::Float(value) => value.base10_parse().ok().map(PLit::Float),
            Lit::Char(value) => Some(PLit::Char(value.value())),
            Lit::Byte(value) => Some(PLit::Int(i128::from(value.value()))),
            _ => None,
        },
        Expr::Unary(u) if matches!(u.op, syn::UnOp::Neg(_)) => match endpoint_lit(&u.expr) {
            Some(PLit::Int(n)) => Some(PLit::Int(-n)),
            Some(PLit::Float(f)) => Some(PLit::Float(-f)),
            _ => None,
        },
        Expr::Paren(p) => endpoint_lit(&p.expr),
        Expr::Group(g) => endpoint_lit(&g.expr),
        Expr::Path(p) if p.path.segments.len() == 2 => {
            let ty = p.path.segments[0].ident.to_string();
            let which = p.path.segments[1].ident.to_string();
            int_type_bound(&ty, &which).map(PLit::Int)
        }
        _ => None,
    }
}

/// Bounds outside i64 clamp to its range, which acts as unbounded.
fn int_type_bound(ty: &str, which: &str) -> Option<i128> {
    let width = IntWidth::parse(ty)?;
    match which {
        "MIN" => Some(width.min()),
        "MAX" => Some(width.max()),
        _ => None,
    }
}

fn lower_literal(literal: &Lit) -> PPat {
    match literal {
        Lit::Int(value) => value
            .base10_parse()
            .map_or(PPat::Unsupported, |value| PPat::Lit(PLit::Int(value))),
        Lit::Float(value) => value
            .base10_parse()
            .map_or(PPat::Unsupported, |value| PPat::Lit(PLit::Float(value))),
        Lit::Bool(value) => PPat::Lit(PLit::Bool(value.value)),
        Lit::Str(value) => PPat::Lit(PLit::Str(value.value())),
        Lit::Char(value) => PPat::Lit(PLit::Char(value.value())),
        Lit::Byte(value) => PPat::Lit(PLit::Int(i128::from(value.value()))),
        _ => PPat::Unsupported,
    }
}

/// `tokio::spawn` or `tokio::task::spawn`.
fn is_tokio_spawn(path: &syn::Path) -> bool {
    let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
    segs.last().map(String::as_str) == Some("spawn") && segs.iter().any(|s| s == "tokio")
}

fn turbofish_collect_target(tf: &syn::AngleBracketedGenericArguments) -> Option<CollectTarget> {
    tf.args.iter().find_map(|arg| match arg {
        syn::GenericArgument::Type(ty) => CollectTarget::of_type(ty),
        _ => None,
    })
}

/// One op, so a `Vec::new()` in a loop skips the path dispatch.
enum EmptyKind {
    Vec,
    Str,
    Map,
    Set,
}

fn empty_container(id: PathId) -> Option<EmptyKind> {
    Some(match id {
        PathId::VecNew | PathId::VecWithCapacity => EmptyKind::Vec,
        PathId::StringNew | PathId::StringWithCapacity => EmptyKind::Str,
        PathId::HashMapNew | PathId::HashMapWithCapacity | PathId::BTreeMapNew => EmptyKind::Map,
        PathId::HashSetNew | PathId::HashSetWithCapacity | PathId::BTreeSetNew => EmptyKind::Set,
        _ => return None,
    })
}

impl Compiler<'_> {
    /// `field: Default::default()` takes the type from the struct definition.
    fn field_default_hint(&mut self, e: &Expr, def: Option<&syn::ItemStruct>, fname: &str) {
        if let Some(call) = bare_default_call(e)
            && let Some(field_ty) = def.and_then(|def| {
                def.fields
                    .iter()
                    .find(|f| f.ident.as_ref().is_some_and(|i| i == fname))
                    .map(|f| f.ty.clone())
            })
            && let Some(ir) = self.default_ir(&field_ty)
        {
            self.default_calls.insert(std::ptr::from_ref(call), ir);
        }
    }

    fn rest_default_hint(&mut self, rest: &Expr, self_type: Option<&str>, path: &syn::Path) {
        if let Some(call) = bare_default_call(rest)
            && let Some(canon) = self_type
                .map(Arc::<str>::from)
                .or_else(|| self.ctx.resolver.resolve_struct_key(self.ctx.module, path))
            && let Some(ir) = self.default_ir_for_struct(&canon)
        {
            self.default_calls.insert(std::ptr::from_ref(call), ir);
        }
    }
}

/// Declaration order when the struct is known, with the serde rename of
/// each field. With a `..rest` every declared field is listed.
fn literal_field_order(
    def: Option<&syn::ItemStruct>,
    written: &[(String, &Expr)],
    has_rest: bool,
) -> (Vec<String>, Vec<Option<Arc<str>>>) {
    match def {
        Some(def) => {
            let mut ordered: Vec<String> = def
                .fields
                .iter()
                .filter_map(|f| f.ident.as_ref().map(std::string::ToString::to_string))
                .filter(|k| has_rest || written.iter().any(|(w, _)| w == k))
                .collect();
            for (k, _) in written {
                if !ordered.contains(k) {
                    ordered.push(k.clone());
                }
            }
            // So a serialized literal uses the same json keys as deserialize.
            let renames = ordered
                .iter()
                .map(|k| {
                    def.fields
                        .iter()
                        .find(|f| f.ident.as_ref().is_some_and(|i| i == k))
                        .and_then(serde_rename)
                        .map(Arc::<str>::from)
                })
                .collect();
            (ordered, renames)
        }
        None => (written.iter().map(|(k, _)| k.clone()).collect(), Vec::new()),
    }
}

/// Parentheses stripped.
pub(super) fn bare_default_call(e: &Expr) -> Option<&syn::ExprCall> {
    match e {
        Expr::Paren(p) => bare_default_call(&p.expr),
        Expr::Group(g) => bare_default_call(&g.expr),
        Expr::Call(c) if c.args.is_empty() => {
            let Expr::Path(p) = &*c.func else { return None };
            let segs: Vec<String> = p
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            (p.qself.is_none() && segs == ["Default", "default"]).then_some(c)
        }
        _ => None,
    }
}
