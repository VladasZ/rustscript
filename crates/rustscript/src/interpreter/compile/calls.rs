//! Calls, closures, assignment, struct literals, and patterns. Split from the compiler.

use std::sync::Arc;

use anyhow::{Result, bail};
use syn::{Expr, Lit, Pat, UnOp};

use crate::interpreter::bytecode::StructShape;
use crate::interpreter::bytecode::{
    BinKind, CapSource, DISCARD, Member, Op, PLit, PPat, PatInfo, Reg, ScalarTy, StructLit,
};
use crate::interpreter::numeric::IntWidth;
use crate::interpreter::serde_attrs::serde_rename;

use super::expr::annotation_scalar;
use super::{
    CollectTarget, Compiler, FnState, HashMap, NameLoc, Res, TypeIr, collect_pattern_names,
    first_generic_type, idx16, int_literal, numeric_annotation,
};

impl Compiler<'_> {
    /// Compile arguments into a fresh contiguous register window and return its
    /// base. The window is reserved first so an argument's own temporaries,
    /// allocated above it, cannot break the packing.
    pub(super) fn compile_args<'e>(&mut self, args: impl Iterator<Item = &'e Expr>) -> Result<Reg> {
        let list: Vec<&Expr> = args.collect();
        let base = self.cur().reg_top;
        for _ in 0..list.len() {
            self.alloc();
        }
        for (i, a) in list.iter().enumerate() {
            self.compile_into(base + idx16(i), a)?;
        }
        Ok(base)
    }

    /// Record the turbofish type args on a call path, e.g. the `AppList` in
    /// `get_json::<AppList>(..)`, returning an index into the current chunk's
    /// `call_type_args` table, or `u32::MAX` when there are none.
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
        // tokio::spawn(async { .. }) lowers to a Spawn op carrying the async
        // block as a child chunk, so the task runs on its own worker thread.
        if self.ctx.async_mode && is_tokio_spawn(path) {
            match c.args.first() {
                Some(Expr::Async(block)) if c.args.len() == 1 => {
                    return self.compile_spawn(dst, &block.block);
                }
                _ => bail!("tokio::spawn needs an async block in this interpreter"),
            }
        }
        let coerce = self.call_coerce(c, path);
        let argc = idx16(c.args.len());

        if self.try_compile_closure_call(dst, c, path, argc)? {
            return Ok(());
        }
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        let resolved = match self.resolve_path_res(&segs) {
            Ok(r) => r,
            Err(_) => Res::External(segs.clone()),
        };
        let path_segs = match resolved {
            // A known function, called directly by id. Turbofish type args are
            // recorded so the callee can bind them to its generic parameters.
            Res::Fn(idx) => {
                let targ = self.record_call_type_args(path);
                let base = self.compile_args(c.args.iter())?;
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
            // A tuple struct constructor.
            Res::Struct(canon) => vec![canon.to_string()],
            // An associated function, UFCS method, or tuple enum variant.
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
                segs
            }
            // A tuple struct built through a type alias, `type P = Point; P(..)`.
            Res::Alias(m, target) => {
                let aliased = match &*target {
                    syn::Type::Path(p) => self.ctx.resolver.resolve_struct_key(m, &p.path),
                    _ => None,
                };
                match aliased {
                    Some(canon) => vec![canon.to_string()],
                    None => bail!("cannot call `{}`", segs.join("::")),
                }
            }
            Res::Enum(_) | Res::Module | Res::Const(_) => {
                bail!("cannot call `{}`", segs.join("::"))
            }
            // Everything else, resolved by the VM through the bridge dispatch.
            Res::External(segs) => {
                if is_transparent_new(&segs) && c.args.len() == 1 {
                    return self.compile_into(dst, &c.args[0]);
                }
                segs
            }
        };
        let p = self.add_path(path_segs, coerce);
        let base = self.compile_args(c.args.iter())?;
        self.emit(Op::CallPath {
            dst,
            path: p,
            base,
            argc,
        });
        Ok(())
    }

    /// The coercion target of a call: its own turbofish, a pending `let`
    /// annotation attached to exactly this call, or the enclosing signature
    /// when the function hands this parse back. See `Compiler::json_let` and
    /// `Compiler::json_tails`.
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

    /// A local or captured closure value called directly by its bare name.
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
        // `v[a..b].copy_from_slice(src)` must write through to `v`. Indexing
        // with a range builds a copied temporary, so the call is compiled
        // against the base vec with the bounds as leading arguments instead.
        // An open end becomes the max sentinel the bridge clamps to the len.
        if m.method == "copy_from_slice" {
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
            return Ok(());
        }
        // A zero-argument `take` is `Option::take` and must empty its place.
        // It used to fall through to the generic clone answer, which left
        // the source Some, a silent wrong answer that made
        // `child.stdin.take()` keep the pipe.
        if m.method == "take"
            && m.args.is_empty()
            && m.turbofish.is_none()
            && self.lower_option_take(dst, &m.receiver)?
        {
            return Ok(());
        }
        // Fuse `x.get(k).copied().unwrap_or(d)` into one op. The chain builds
        // and tears down an Option per call, which dominates counting loops.
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
        // An `unwrap_or_default` whose own result is unwrapped again must have
        // produced an `Option`, so its default is `None`. That is a fact about
        // the shape of the chain, not a guess about the type, and it is the
        // only thing that can type the inner call of
        // `x.unwrap_or_default().unwrap_or_default()`.
        let outer_option_hint = self.option_result.take();
        if m.method == "unwrap_or_default"
            && let Expr::MethodCall(inner) = &*m.receiver
            && inner.method == "unwrap_or_default"
        {
            self.option_result = Some(std::ptr::from_ref(inner));
        }
        let recv = self.compile_expr(&m.receiver)?;
        self.option_result = outer_option_hint;
        let base = self.compile_args(m.args.iter())?;
        let (method, scalar) = self.method_name_and_scalar(m);
        let name = self.add_name_with(method, scalar);
        // A multiline chain compiles its receiver and args first, so restamp
        // with the method's own line before the op lands, the line rustc
        // would name for this call.
        self.set_line(m.method.span());
        self.emit(Op::Method {
            dst,
            recv,
            name,
            base,
            argc: idx16(m.args.len()),
        });
        // Methods that fill a `&mut` argument, like read_line, write the new
        // value into the arg window. The window slot is only a copy of the
        // variable, so move the result back into the variable register.
        self.emit_mut_arg_writebacks(m.args.iter(), base)?;
        Ok(())
    }

    /// The lowered method name and its scalar result type, where knowable.
    /// `collect` is type driven in real Rust; the interpreter has no types, so
    /// the three places the target is knowable rename the call to
    /// `collect_string`: a turbofish asking for a String, a pending
    /// `let s: String` annotation attached to exactly this call, and a
    /// `-> String` signature on the function whose returned value this call
    /// produces. See `Compiler::string_let` and `Compiler::string_tails`.
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
                // Cleared only when this call consumed the pending `let`
                // hint. A turbofish collect nested inside the annotated
                // chain, say in one branch of its `if`, resolves through
                // its own turbofish, and clearing here made the outer
                // collect fall back to a vec of pairs.
                if from_let.is_some() {
                    self.collect_let = None;
                }
                method = target.method_name().to_string();
            }
        }
        // An explicit turbofish is the only place a method's result type is
        // written down, so it rides on the name for the methods that need it.
        let mut scalar = turbofish_scalar(m.turbofish.as_ref());
        // `unwrap_or_default` carries no turbofish of its own, its type is the
        // payload of the Option it is called on. Wherever the source states
        // that payload, as `None::<u64>` or `then_some(1u8)` do, the receiver
        // is where it appears, and without it the default fell back to an
        // empty string whatever the real type was.
        if scalar.is_none() && m.method == "unwrap_or_default" {
            let env = TyEnv {
                locals: &self.typed_locals,
                fn_returns: self.ctx.fn_returns,
            };
            scalar = option_payload(&m.receiver, &env);
        }
        // A pending `let x: T = ...unwrap_or_default()` annotation names the
        // payload of the outermost call in the chain.
        if scalar.is_none()
            && let Some((ptr, ty)) = &self.default_let
            && std::ptr::eq(*ptr, m)
        {
            scalar = Some(ty.clone());
            self.default_let = None;
        }
        // Failing all of that, this call's own result is unwrapped again, so
        // whatever it holds, it produced an Option and defaults to None.
        if matches!(self.option_result, Some(ptr) if std::ptr::eq(ptr, m)) {
            self.option_result = None;
            scalar = scalar.or(Some(ScalarTy::Opt(Box::new(ScalarTy::Other))));
        }
        (method, scalar)
    }

    /// The type an expression states about itself, read against the current
    /// typed locals. Lets `compile_let` record an unannotated
    /// `let sorted = vec!['a', 'b']` so a later default built from `sorted`'s
    /// elements has the right type.
    pub(super) fn stated_ty(&self, expr: &Expr) -> Option<ScalarTy> {
        let env = TyEnv {
            locals: &self.typed_locals,
            fn_returns: self.ctx.fn_returns,
        };
        written_ty(expr, &env)
    }

    /// Emit a writeback for every `&mut variable` argument of a finished call.
    /// The callee worked on the arg window copy, and the VM hands the final
    /// values back into that window on return, so a move from the window slot
    /// lands the mutation in the caller's variable. Only calls whose window
    /// survives the call may use this, a `CallPath` consumes its args instead.
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
            }
        }
        Ok(())
    }

    /// Compile an `async { .. }` block from `tokio::spawn` into a zero argument
    /// child chunk and emit a Spawn op. Captures work like a closure's.
    fn compile_spawn(&mut self, dst: Reg, block: &syn::Block) -> Result<()> {
        self.frames.push(FnState::new("<task>".to_string()));
        self.cur().num_params = 0;
        let ret = self.alloc();
        self.compile_block(block, ret)?;
        self.emit(Op::Ret { src: ret });
        let child = self.frames.pop().unwrap();
        let caps: Vec<CapSource> = child.upvalues.iter().map(|(_, s)| *s).collect();
        let mut chunk = child.into_chunk(self.ctx.file.clone());
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
            // An annotated param is a type the program wrote down, recorded
            // like an annotated let, so a default built from the param in the
            // closure body reads the right type.
            if let Pat::Type(t) = p
                && let Pat::Ident(id) = &*t.pat
                && let Some(declared) = annotation_scalar(&t.ty)
            {
                self.typed_locals.insert(id.ident.to_string(), declared);
            }
            match p {
                Pat::Ident(id) if id.subpat.is_none() => self.define(&id.ident.to_string(), reg),
                _ => self.bind_pattern_irrefutable(p, reg)?,
            }
            // A numeric param annotation retags the incoming value, the same
            // rule as a fn param, so the body computes in the stated width.
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
        let mut chunk = child.into_chunk(self.ctx.file.clone());
        chunk.module = idx16(self.ctx.module);
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

    /// Rust evaluates an assignment's right operand before the place, so a
    /// panic in the value fires before a panic in the place expression.
    /// Lower a zero-argument `take` on a place expression: read the place
    /// into `dst`, then store None back through the same registers, so every
    /// part of the place is evaluated exactly once. True when the receiver
    /// was such a place. A temporary receiver answers false and keeps the
    /// generic clone answer, which matches real Rust observably, the emptied
    /// temporary is gone either way.
    fn lower_option_take(&mut self, dst: Reg, place: &Expr) -> Result<bool> {
        match place {
            Expr::Path(p) if p.qself.is_none() && p.path.segments.len() == 1 => {
                let name = p.path.segments[0].ident.to_string();
                if dst != DISCARD {
                    self.load_name(&name, dst)?;
                }
                let location = self.resolve_for_write(&name);
                let none = self.alloc();
                self.load_name("None", none)?;
                self.emit_name_store(location, none, &name)?;
                Ok(true)
            }
            Expr::Field(f) => {
                let base = self.compile_expr(&f.base)?;
                let member = self.member_of(&f.member);
                if dst != DISCARD {
                    self.emit(Op::GetField { dst, base, member });
                }
                let none = self.alloc();
                self.load_name("None", none)?;
                self.emit(Op::SetField {
                    base,
                    member,
                    val: none,
                });
                Ok(true)
            }
            Expr::Index(ix) => {
                let base = self.compile_expr(&ix.expr)?;
                let key = self.compile_expr(&ix.index)?;
                if dst != DISCARD {
                    self.emit(Op::Index { dst, base, key });
                }
                let none = self.alloc();
                self.load_name("None", none)?;
                self.emit(Op::SetIndex {
                    base,
                    key,
                    val: none,
                });
                Ok(true)
            }
            Expr::Paren(p) => self.lower_option_take(dst, &p.expr),
            _ => Ok(false),
        }
    }

    /// The register of a `&mut` parameter named as a bare deref target,
    /// `*seq` for a `seq: &mut usize` parameter of the current function.
    /// Only a plain local parameter qualifies. A cell-promoted or captured
    /// name keeps the strict reference-only op.
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

    pub(super) fn compile_assign(&mut self, target: &Expr, value: &Expr) -> Result<()> {
        match target {
            Expr::Path(p) if p.path.segments.len() == 1 => {
                let name = p.path.segments[0].ident.to_string();
                let location = self.resolve_for_write(&name);
                let value = self.compile_expr(value)?;
                self.emit_name_store(location, value, &name)?;
            }
            Expr::Index(idx) => {
                let val = self.compile_expr(value)?;
                let base = self.compile_expr(&idx.expr)?;
                let key = self.compile_expr(&idx.index)?;
                self.emit(Op::SetIndex { base, key, val });
            }
            Expr::Field(f) => {
                let val = self.compile_expr(value)?;
                let base = self.compile_expr(&f.base)?;
                let member = self.member_of(&f.member);
                self.emit(Op::SetField { base, member, val });
            }
            Expr::Unary(u) if matches!(u.op, UnOp::Deref(_)) => {
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

    /// Rust evaluates a compound assignment's right operand before the place,
    /// so a panic in the value fires before a panic in the place expression.
    pub(super) fn compile_compound_assign(
        &mut self,
        target: &Expr,
        op: BinKind,
        rhs: &Expr,
    ) -> Result<()> {
        // `a op= b` becomes `a = a op b`.
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
                let base = self.compile_expr(&idx.expr)?;
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
                let base = self.compile_expr(&f.base)?;
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

    /// `*target op= rhs`, the deref arm of `compile_compound_assign`.
    fn compile_compound_deref_assign(
        &mut self,
        u: &syn::ExprUnary,
        op: BinKind,
        rhs: &Expr,
    ) -> Result<()> {
        let b = self.compile_expr(rhs)?;
        let param = self.deref_param_reg(&u.expr);
        let target = self.compile_expr(&u.expr)?;
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
        match param {
            Some(target) => self.emit(Op::SetDerefParam {
                target,
                val: result,
            }),
            None => self.emit(Op::SetDeref {
                target,
                val: result,
            }),
        }
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
            syn::Member::Named(n) => self.add_member(Member::Named(n.to_string().into())),
            syn::Member::Unnamed(i) => self.add_member(Member::Indexed(i.index as usize)),
        }
    }

    pub(super) fn compile_struct_literal(&mut self, dst: Reg, s: &syn::ExprStruct) -> Result<()> {
        // A user struct resolves to its canonical name, which keys shapes,
        // methods, and coercions. Anything else, an enum struct variant for
        // example, keeps the bare last segment.
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
        // Written fields keyed by name.
        let mut written: Vec<(String, &Expr)> = Vec::new();
        for f in &s.fields {
            let key = match &f.member {
                syn::Member::Named(n) => n.to_string(),
                syn::Member::Unnamed(i) => i.index.to_string(),
            };
            written.push((key, &f.expr));
        }
        // Field order follows the declaration when the struct is known.
        // Written fields in declaration order, then any extras. A trailing
        // `..rest` fills whatever was not written.
        let (order, renames): (Vec<String>, Vec<Option<Arc<str>>>) = match def {
            Some(def) => {
                let mut ordered: Vec<String> = def
                    .fields
                    .iter()
                    .filter_map(|f| f.ident.as_ref().map(std::string::ToString::to_string))
                    .filter(|k| written.iter().any(|(w, _)| w == k))
                    .collect();
                for (k, _) in &written {
                    if !ordered.contains(k) {
                        ordered.push(k.clone());
                    }
                }
                // One rename slot per ordered field, read from the struct def so
                // a serialized literal uses the same json keys as deserialize.
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
        };
        // Reserve a packed window, then fill it, so field temporaries do not
        // break the packing.
        let has_rest = s.rest.is_some();
        let slots = order.len() + usize::from(has_rest);
        let base = self.cur().reg_top;
        for _ in 0..slots {
            self.alloc();
        }
        for (i, fname) in order.iter().enumerate() {
            let dstf = base + idx16(i);
            match written.iter().find(|(k, _)| k == fname) {
                Some((_, e)) => self.compile_into(dstf, e)?,
                None => self.emit(Op::LoadUnit { dst: dstf }),
            }
        }
        if let Some(rest) = &s.rest {
            self.compile_into(base + idx16(order.len()), rest)?;
        }
        let info = {
            let shape = StructShape::with_renames(
                name,
                order.into_iter().map(Into::into).collect(),
                renames,
            );
            let f = self.cur();
            f.struct_lits.push(StructLit { shape, has_rest });
            idx16(f.struct_lits.len() - 1)
        };
        self.emit(Op::MakeStruct { dst, info, base });
        Ok(())
    }

    // -- patterns ----------------------------------------------------------

    /// Register a pattern and the slot each bound name uses.
    pub(super) fn pattern_info(&mut self, pat: &Pat) -> Result<u16> {
        let mut names = Vec::new();
        collect_pattern_names(pat, &mut names);
        let mut binds = Vec::new();
        for n in names {
            let reg = self.alloc();
            self.define(&n, reg);
            binds.push((n, reg));
        }
        let f = self.cur();
        f.pats.push(PatInfo {
            pat: lower_pattern(pat),
            binds,
        });
        Ok(u16::try_from(f.pats.len() - 1)?)
    }

    /// Bind an irrefutable pattern whose value already sits in `reg`.
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
                // Tuple or struct destructuring, use a test-and-bind that always
                // matches for these irrefutable shapes.
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

fn is_transparent_new(segments: &[String]) -> bool {
    let Some((prefix, [receiver, method])) = segments.split_last_chunk::<2>() else {
        return false;
    };
    (prefix.is_empty() || matches!(prefix.first().map(String::as_str), Some("std" | "alloc")))
        && method == "new"
        && matches!(receiver.as_str(), "Box" | "Rc" | "Arc" | "RefCell" | "Cell")
}

// A bare identifier pattern that names a unit variant, not a new binding. Real Rust tells the two
// apart by name resolution, which we do not have, so we lean on the naming rule these scripts
// follow. Bindings are snake_case and variants are UpperCamel. So an uppercase-initial ident with no
// ref, mut, or subpattern is a unit-variant pattern like None, not a binding. Without this a bare
// None arm lowers to an always-true catch-all and matches a Some value.
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

fn lower_pattern(pattern: &Pat) -> PPat {
    match pattern {
        Pat::Wild(_) => PPat::Wild,
        Pat::Rest(_) => PPat::Rest,
        Pat::Ident(ident) if is_unit_variant_ident(ident) => PPat::Path {
            name: Some(ident.ident.to_string()),
        },
        Pat::Ident(ident) => PPat::Ident {
            name: ident.ident.to_string(),
            sub: ident
                .subpat
                .as_ref()
                .map(|subpattern| Box::new(lower_pattern(&subpattern.1))),
        },
        Pat::Lit(literal) => lower_literal(&literal.lit),
        Pat::Paren(paren) => lower_pattern(&paren.pat),
        Pat::Reference(reference) => lower_pattern(&reference.pat),
        Pat::Type(typed) => lower_pattern(&typed.pat),
        Pat::Tuple(tuple) => PPat::Tuple(tuple.elems.iter().map(lower_pattern).collect()),
        Pat::TupleStruct(tuple) => PPat::TupleStruct {
            name: tuple
                .path
                .segments
                .last()
                .map(|segment| segment.ident.to_string()),
            elems: tuple.elems.iter().map(lower_pattern).collect(),
        },
        Pat::Path(path) => PPat::Path {
            name: path
                .path
                .segments
                .last()
                .map(|segment| segment.ident.to_string()),
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
                    (name, lower_pattern(&field.pat))
                })
                .collect(),
        },
        Pat::Or(or) => PPat::Or(or.cases.iter().map(lower_pattern).collect()),
        Pat::Slice(slice) => PPat::Slice(slice.elems.iter().map(lower_pattern).collect()),
        Pat::Range(range) => lower_range(range),
        _ => PPat::Unsupported,
    }
}

fn lower_range(range: &syn::PatRange) -> PPat {
    // Outer None means a present endpoint that is not a supported literal,
    // inner None means that side of the range is unbounded.
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

/// A literal range endpoint, including a negated number, seen through parens.
fn endpoint_lit(e: &Expr) -> Option<PLit> {
    match e {
        Expr::Lit(l) => match &l.lit {
            Lit::Int(value) => value.base10_parse().ok().map(PLit::Int),
            Lit::Float(value) => value.base10_parse().ok().map(PLit::Float),
            Lit::Char(value) => Some(PLit::Char(value.value())),
            Lit::Byte(value) => Some(PLit::Int(i64::from(value.value()))),
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

/// The `MIN` or `MAX` associated const of an integer type, as the i64 the
/// interpreter stores every integer in. Bounds outside i64, the u64 and u128
/// maxima, clamp to i64's range, which acts as unbounded for stored values.
fn int_type_bound(ty: &str, which: &str) -> Option<i64> {
    let (lo, hi) = match ty {
        "i8" => (i64::from(i8::MIN), i64::from(i8::MAX)),
        "i16" => (i64::from(i16::MIN), i64::from(i16::MAX)),
        "i32" => (i64::from(i32::MIN), i64::from(i32::MAX)),
        "i64" | "isize" | "i128" => (i64::MIN, i64::MAX),
        "u8" => (0, i64::from(u8::MAX)),
        "u16" => (0, i64::from(u16::MAX)),
        "u32" => (0, i64::from(u32::MAX)),
        "u64" | "usize" | "u128" => (0, i64::MAX),
        _ => return None,
    };
    match which {
        "MIN" => Some(lo),
        "MAX" => Some(hi),
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
        Lit::Byte(value) => PPat::Lit(PLit::Int(i64::from(value.value()))),
        _ => PPat::Unsupported,
    }
}

/// Whether a call path names tokio's `spawn`, either `tokio::spawn` or
/// `tokio::task::spawn`.
fn is_tokio_spawn(path: &syn::Path) -> bool {
    let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
    segs.last().map(String::as_str) == Some("spawn") && segs.iter().any(|s| s == "tokio")
}

/// The collect target a turbofish asks for, as in `collect::<String>()` or
/// `collect::<HashMap<K, V>>()`.
fn turbofish_collect_target(tf: &syn::AngleBracketedGenericArguments) -> Option<CollectTarget> {
    tf.args.iter().find_map(|arg| match arg {
        syn::GenericArgument::Type(ty) => CollectTarget::of_type(ty),
        _ => None,
    })
}

/// The type facts a payload walk can read: the declared types of annotated
/// locals and the stated return scalars of the script's own functions.
struct TyEnv<'a> {
    locals: &'a HashMap<String, ScalarTy>,
    fn_returns: &'a HashMap<String, ScalarTy>,
}

/// The payload type of an expression that syntactically builds an `Option`,
/// for the cases where the source states it outright. Only a `Default` is ever
/// built from this, so a container answers with the kind of default it has
/// rather than with its element type.
///
/// This is not type inference. Every arm reads a type the program wrote down,
/// and anything else answers `None` so the caller keeps its old behavior.
fn option_payload(expr: &Expr, env: &TyEnv) -> Option<ScalarTy> {
    match expr {
        Expr::Paren(inner) => option_payload(&inner.expr, env),
        Expr::Group(inner) => option_payload(&inner.expr, env),
        // A block answers through its tail expression.
        Expr::Block(block) => block_tail(&block.block).and_then(|e| option_payload(e, env)),
        // An if-else answers through whichever branch states its type,
        // `if c { Some(x as i16) } else { None::<i16> }` from either side.
        Expr::If(sel) => block_tail(&sel.then_branch)
            .and_then(|e| option_payload(e, env))
            .or_else(|| {
                sel.else_branch
                    .as_ref()
                    .and_then(|(_, e)| option_payload(e, env))
            }),
        Expr::Path(path) => {
            let segment = path.path.segments.last()?;
            // `None::<T>`, the payload is the turbofish.
            if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                return turbofish_scalar(Some(args));
            }
            // A bare name the program declared as `let opt: Option<T>`.
            match env.locals.get(&segment.ident.to_string()) {
                Some(ScalarTy::Opt(payload)) => Some((**payload).clone()),
                _ => None,
            }
        }
        // `Some(x)`, the payload is whatever `x` is.
        Expr::Call(call) => {
            let Expr::Path(path) = &*call.func else {
                return None;
            };
            let last = path.path.segments.last()?;
            (last.ident == "Some")
                .then(|| call.args.first().and_then(|a| written_ty(a, env)))
                .flatten()
        }
        Expr::MethodCall(call) => match call.method.to_string().as_str() {
            // `flag.then_some(x)` is an `Option` of whatever `x` is.
            "then_some" => call.args.first().and_then(|a| written_ty(a, env)),
            // `text.parse::<T>()` states its payload in its own turbofish.
            "parse" => turbofish_scalar(call.turbofish.as_ref()),
            // `a.or(b)` keeps the payload both sides share, so either side
            // that states it answers for both.
            "or" => call
                .args
                .first()
                .and_then(|a| option_payload(a, env))
                .or_else(|| option_payload(&call.receiver, env)),
            // These hand the same payload through untouched. `ok` moves a
            // `Result` payload into an `Option`, the same layer.
            "clone" | "cloned" | "copied" | "take" | "as_ref" | "as_mut" | "filter" | "ok" => {
                option_payload(&call.receiver, env)
            }
            // `x.unwrap_or_default()`, `unwrap`, and `expect` peel one layer,
            // so their own payload is one layer further in than the receiver's.
            "unwrap_or_default" | "unwrap" | "expect" => {
                option_payload(&call.receiver, env)?.payload().cloned()
            }
            // `x.unwrap_or(d)` peels one layer the same way, and the fallback
            // argument states the same type when the receiver does not.
            "unwrap_or" => option_payload(&call.receiver, env)
                .and_then(|payload| payload.payload().cloned())
                .or_else(|| call.args.first().and_then(|a| option_payload(a, env))),
            // `v.get(i)`, the accessors, and the no-argument iterator
            // reductions answer an `Option` of the vec's element type.
            // `map.get(k)` answers an `Option` of the map's value type.
            "get" => element_ty(&call.receiver, env).or_else(|| map_value_ty(&call.receiver, env)),
            // `map.remove(k)` answers an `Option` of the map's value type. A
            // vec's `remove` answers its element outright, not an `Option`,
            // and a map receiver is the only kind this walk answers for.
            "remove" => map_value_ty(&call.receiver, env),
            // `ch.to_digit(radix)` answers an `Option<u32>` whatever the
            // receiver, the one char method with an `Option` payload.
            "to_digit" => Some(ScalarTy::Int(IntWidth::U32)),
            "first" | "last" | "pop" => element_ty(&call.receiver, env),
            "min" | "max" if call.args.is_empty() => element_ty(&call.receiver, env),
            // `x.checked_add(y)` answers an `Option` of the receiver's own
            // integer width.
            "checked_add" | "checked_sub" | "checked_mul" | "checked_div" | "checked_rem"
            | "checked_neg" | "checked_abs" | "checked_pow" | "checked_shl" | "checked_shr"
            | "checked_div_euclid" | "checked_rem_euclid" => {
                match written_ty(&call.receiver, env) {
                    Some(ty @ ScalarTy::Int(_)) => Some(ty),
                    _ => None,
                }
            }
            _ => None,
        },
        _ => None,
    }
}

/// The tail expression of a block, when the block ends in one.
fn block_tail(block: &syn::Block) -> Option<&Expr> {
    match block.stmts.last()? {
        syn::Stmt::Expr(expr, None) => Some(expr),
        _ => None,
    }
}

/// The element type of an expression that syntactically builds a `Vec`, for
/// the same narrow purpose as `option_payload`, and just as literally: every
/// arm reads a type the program wrote down.
fn element_ty(expr: &Expr, env: &TyEnv) -> Option<ScalarTy> {
    match expr {
        Expr::Paren(inner) => element_ty(&inner.expr, env),
        Expr::Group(inner) => element_ty(&inner.expr, env),
        Expr::Block(block) => block_tail(&block.block).and_then(|e| element_ty(e, env)),
        // An if-else answers through whichever branch states its element.
        Expr::If(sel) => block_tail(&sel.then_branch)
            .and_then(|e| element_ty(e, env))
            .or_else(|| {
                sel.else_branch
                    .as_ref()
                    .and_then(|(_, e)| element_ty(e, env))
            }),
        // A bare name the program declared as `let v: Vec<T>`.
        Expr::Path(path) => {
            let segment = path.path.segments.last()?;
            match env.locals.get(&segment.ident.to_string()) {
                Some(ScalarTy::List(element)) => Some((**element).clone()),
                _ => None,
            }
        }
        // A `vec![..]` literal states its element type through any element
        // that states its own.
        Expr::Macro(mac) if mac.mac.path.is_ident("vec") => vec_macro_element(&mac.mac, env),
        // `Vec::<T>::new()` states it in the turbofish.
        Expr::Call(_) => match written_ty(expr, env) {
            Some(ScalarTy::List(element)) => Some(*element),
            _ => None,
        },
        Expr::MethodCall(call) => match call.method.to_string().as_str() {
            // These pass elements through unchanged.
            "iter" | "into_iter" | "cloned" | "copied" | "clone" | "to_vec" | "rev" => {
                element_ty(&call.receiver, env)
            }
            // `it.map(|x| e)` makes whatever `e` states its own type to be
            // the element type.
            "map" => match call.args.first() {
                Some(Expr::Closure(closure)) => written_ty(&closure.body, env),
                _ => None,
            },
            // `it.collect::<Vec<T>>()` states its element in the turbofish.
            "collect" => match turbofish_scalar(call.turbofish.as_ref()) {
                Some(ScalarTy::List(element)) => Some(*element),
                _ => None,
            },
            // The vec an unwrap settles on, from whichever side wrote its
            // type down.
            "unwrap" | "unwrap_or" | "unwrap_or_default" => {
                let from_receiver = match option_payload(&call.receiver, env) {
                    Some(ScalarTy::List(element)) => Some(*element),
                    _ => None,
                };
                from_receiver.or_else(|| call.args.first().and_then(|a| element_ty(a, env)))
            }
            _ => None,
        },
        _ => None,
    }
}

/// The stated element type of a `vec![..]` literal, from the first element
/// that states one. The repeat form `vec![x; n]` answers through `x`.
fn vec_macro_element(mac: &syn::Macro, env: &TyEnv) -> Option<ScalarTy> {
    use syn::Token;
    use syn::punctuated::Punctuated;
    if let Ok(elements) = mac.parse_body_with(Punctuated::<Expr, Token![,]>::parse_terminated) {
        return elements.iter().find_map(|e| written_ty(e, env));
    }
    mac.parse_body_with(Punctuated::<Expr, Token![;]>::parse_terminated)
        .ok()?
        .first()
        .and_then(|e| written_ty(e, env))
}

/// A method whose answer has its receiver's own type. `clone` hands it
/// through untouched, the ASCII case methods keep char as char and u8 as u8,
/// and the arithmetic methods keep their receiver's width, which is how
/// `(x as u8).saturating_mul(y)` in a map closure states a u8 element.
fn keeps_receiver_ty(method: &str) -> bool {
    matches!(
        method,
        "clone"
            | "to_ascii_lowercase"
            | "to_ascii_uppercase"
            | "saturating_add"
            | "saturating_sub"
            | "saturating_mul"
            | "wrapping_add"
            | "wrapping_sub"
            | "wrapping_mul"
            | "rotate_left"
            | "rotate_right"
            | "rem_euclid"
            | "div_euclid"
            | "pow"
            | "powi"
            | "powf"
            | "abs"
            | "signum"
            | "isqrt"
    )
}

/// The type an expression states about itself, for the same narrow purpose.
fn written_ty(expr: &Expr, env: &TyEnv) -> Option<ScalarTy> {
    match expr {
        Expr::Paren(inner) => written_ty(&inner.expr, env),
        Expr::Group(inner) => written_ty(&inner.expr, env),
        // A block answers through its tail expression, which is how a
        // `({ let mut m: HashMap<K, V> = ...; m })` vec element states itself.
        Expr::Block(block) => block_tail(&block.block).and_then(|e| written_ty(e, env)),
        // An if-else answers through whichever branch states its type, so
        // `then_some(if flag { '9' } else { c })` knows it holds a char.
        Expr::If(sel) => block_tail(&sel.then_branch)
            .and_then(|e| written_ty(e, env))
            .or_else(|| {
                sel.else_branch
                    .as_ref()
                    .and_then(|(_, e)| written_ty(e, env))
            }),
        // `value as u8` names the type at the cast.
        Expr::Cast(cast) => ScalarTy::lower(&cast.ty),
        // Arithmetic keeps its operands' type, so either side that states it
        // answers, `(x as i8) / (y as i8)` for one. A comparison is a bool.
        Expr::Binary(bin) => {
            use syn::BinOp::{
                Add, And, BitAnd, BitOr, BitXor, Div, Eq, Ge, Gt, Le, Lt, Mul, Ne, Or, Rem, Shl,
                Shr, Sub,
            };
            match bin.op {
                Add(_) | Sub(_) | Mul(_) | Div(_) | Rem(_) | BitAnd(_) | BitOr(_) | BitXor(_) => {
                    written_ty(&bin.left, env).or_else(|| written_ty(&bin.right, env))
                }
                Shl(_) | Shr(_) => written_ty(&bin.left, env),
                Eq(_) | Ne(_) | Lt(_) | Le(_) | Gt(_) | Ge(_) | And(_) | Or(_) => {
                    Some(ScalarTy::Bool)
                }
                _ => None,
            }
        }
        Expr::Unary(un) => match un.op {
            syn::UnOp::Neg(_) | syn::UnOp::Not(_) => written_ty(&un.expr, env),
            _ => None,
        },
        Expr::Lit(lit) => match &lit.lit {
            Lit::Str(_) => Some(ScalarTy::Str),
            Lit::Bool(_) => Some(ScalarTy::Bool),
            Lit::Char(_) => Some(ScalarTy::Char),
            Lit::Int(int) => IntWidth::parse(int.suffix()).map(ScalarTy::Int),
            Lit::Float(float) => match float.suffix() {
                "f32" => Some(ScalarTy::F32),
                "f64" => Some(ScalarTy::F64),
                _ => None,
            },
            _ => None,
        },
        // These methods answer in their receiver's own type, so the receiver
        // states it for the whole call.
        Expr::MethodCall(call) if keeps_receiver_ty(&call.method.to_string()) => {
            written_ty(&call.receiver, env)
        }
        // `it.collect::<T>()` states its own type in the turbofish, which is
        // how a `map(|x| ...collect::<Vec<bool>>()).min()` chain learns what
        // its default is.
        Expr::MethodCall(call)
            if call.method == "collect" && turbofish_scalar(call.turbofish.as_ref()).is_some() =>
        {
            turbofish_scalar(call.turbofish.as_ref())
        }
        // A fold answers in its init's type, which the accumulator keeps
        // through every step, so `it.fold(0u8, ..).checked_mul(..)` knows
        // its payload width even when the chain runs through a `map`.
        Expr::MethodCall(call) if call.method == "fold" => {
            call.args.first().and_then(|init| written_ty(init, env))
        }
        // An unwrap's own value is the receiver's payload, so
        // `Some('\n').unwrap_or_default()` is a char, not an Option.
        Expr::MethodCall(call)
            if matches!(
                call.method.to_string().as_str(),
                "unwrap" | "expect" | "unwrap_or" | "unwrap_or_default"
            ) && option_payload(&call.receiver, env).is_some() =>
        {
            option_payload(&call.receiver, env)
        }
        // Anything that is itself an `Option` is one layer deeper, keeping
        // what it wraps so a further unwrap can still read it.
        Expr::Call(_) | Expr::Path(_) | Expr::MethodCall(_) => {
            if let Some(payload) = option_payload(expr, env) {
                Some(ScalarTy::Opt(Box::new(payload)))
            } else if is_none_path(expr) {
                Some(ScalarTy::Opt(Box::new(ScalarTy::Other)))
            } else if let Some(element) = vec_new_element(expr) {
                Some(ScalarTy::List(Box::new(element)))
            } else if let Some(container) = container_new_ty(expr) {
                Some(container)
            } else if is_string_call(expr) {
                Some(ScalarTy::Str)
            } else if let Expr::Path(path) = expr
                && path.path.segments.len() == 1
                && let Some(declared) = env.locals.get(&path.path.segments[0].ident.to_string())
            {
                // A bare name the program declared with any scalar
                // annotation, `let x: u16` included.
                Some(declared.clone())
            } else {
                fn_return_ty(expr, env)
            }
        }
        Expr::Macro(mac) if mac.mac.path.is_ident("vec") => Some(ScalarTy::List(Box::new(
            vec_macro_element(&mac.mac, env).unwrap_or(ScalarTy::Other),
        ))),
        _ => None,
    }
}

/// The stated return scalar of a call to one of the script's own functions,
/// `f()` when `fn f() -> f32` says so.
fn fn_return_ty(expr: &Expr, env: &TyEnv) -> Option<ScalarTy> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Expr::Path(path) = &*call.func else {
        return None;
    };
    let segment = path.path.segments.last()?;
    env.fn_returns.get(&segment.ident.to_string()).cloned()
}

/// A call that builds a `String` outright, `String::from(..)` or
/// `String::new()`.
fn is_string_call(expr: &Expr) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    let Expr::Path(path) = &*call.func else {
        return false;
    };
    let mut segments = path.path.segments.iter().rev();
    let is_ctor = segments
        .next()
        .is_some_and(|s| s.ident == "from" || s.ident == "new");
    is_ctor && segments.next().is_some_and(|s| s.ident == "String")
}

/// The stated element type of a `Vec::<T>::new()` or `VecDeque::<T>::new()`
/// call, read from the turbofish on the container segment.
fn vec_new_element(expr: &Expr) -> Option<ScalarTy> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Expr::Path(path) = &*call.func else {
        return None;
    };
    let mut segments = path.path.segments.iter().rev();
    let last = segments.next()?;
    if last.ident != "new" {
        return None;
    }
    let container = segments.next()?;
    if container.ident != "Vec" && container.ident != "VecDeque" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &container.arguments else {
        return None;
    };
    turbofish_scalar(Some(args))
}

/// The map or set type a `HashMap::<K, V>::new()` / `HashSet::<T>::new()`
/// call states in its own turbofish, the map twin of `vec_new_element`.
fn container_new_ty(expr: &Expr) -> Option<ScalarTy> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Expr::Path(path) = &*call.func else {
        return None;
    };
    let mut segments = path.path.segments.iter().rev();
    let last = segments.next()?;
    if last.ident != "new" {
        return None;
    }
    let container = segments.next()?;
    let name = container.ident.to_string();
    if !matches!(
        name.as_str(),
        "HashMap" | "BTreeMap" | "HashSet" | "BTreeSet"
    ) {
        return None;
    }
    ScalarTy::lower_segment(container)
}

/// The value type of an expression that syntactically builds a map, for the
/// `map.get(k)` payload, read as literally as `element_ty`.
fn map_value_ty(expr: &Expr, env: &TyEnv) -> Option<ScalarTy> {
    match expr {
        Expr::Paren(inner) => map_value_ty(&inner.expr, env),
        Expr::Group(inner) => map_value_ty(&inner.expr, env),
        Expr::Block(block) => block_tail(&block.block).and_then(|e| map_value_ty(e, env)),
        // A bare name the program declared as `let m: HashMap<K, V>`.
        Expr::Path(path) => {
            let segment = path.path.segments.last()?;
            match env.locals.get(&segment.ident.to_string()) {
                Some(ScalarTy::Map(value)) => Some((**value).clone()),
                _ => None,
            }
        }
        // `HashMap::<K, V>::new()` states it in the turbofish.
        Expr::Call(_) => match container_new_ty(expr) {
            Some(ScalarTy::Map(value)) => Some(*value),
            _ => None,
        },
        Expr::MethodCall(call) if call.method == "clone" => map_value_ty(&call.receiver, env),
        // `it.collect::<HashMap<K, V>>()` states its value type in the
        // turbofish.
        Expr::MethodCall(call) if call.method == "collect" => {
            match turbofish_scalar(call.turbofish.as_ref()) {
                Some(ScalarTy::Map(value)) => Some(*value),
                _ => None,
            }
        }
        _ => None,
    }
}

/// A bare `None`, with or without a turbofish.
fn is_none_path(expr: &Expr) -> bool {
    matches!(expr, Expr::Path(path)
        if path.path.segments.last().is_some_and(|s| s.ident == "None"))
}

/// The first concrete scalar named by a turbofish argument list.
fn turbofish_scalar(args: Option<&syn::AngleBracketedGenericArguments>) -> Option<ScalarTy> {
    args?
        .args
        .iter()
        .find_map(|arg| match arg {
            syn::GenericArgument::Type(ty) => Some(ty),
            _ => None,
        })
        .and_then(ScalarTy::lower)
}
