//! Calls, closures, assignment, struct literals, and patterns. Split from the compiler.

use std::sync::Arc;

use anyhow::{Result, bail};
use syn::{Expr, Lit, Pat, UnOp};

use crate::interpreter::bytecode::StructShape;
use crate::interpreter::bytecode::{
    BinKind, BuiltinId, CapSource, Const, DISCARD, EnumVariant, FieldName, Member, Op, PLit, PPat,
    PTag, PatInfo, PathId, PathRef, Reg, ScalarTy, StructLit,
};
use crate::interpreter::enum_def::{EnumDef, builtin_enum, prelude_variant};
use crate::interpreter::numeric::IntWidth;
use crate::interpreter::serde_attrs::serde_rename;
use crate::interpreter::typeir::CastIr;

use super::expr::annotation_scalar;
use super::place;
use super::written::{TyEnv, option_payload, turbofish_scalar, written_ty};
use super::{
    CollectTarget, Compiler, FnState, NameLoc, Res, TypeIr, collect_pattern_names,
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
            self.emit_arg_move_out(a);
        }
        Ok(base)
    }

    /// A plain-path by-value argument is a move in real Rust. When the
    /// program has `Drop` impls, clear the binding register after the copy
    /// when the value's type has a user `Drop` impl, so the guard drops
    /// where the move sent it. A reference argument compiles as
    /// `Expr::Reference` and never reaches this.
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
            // A borrow parameter forwards a reference, not ownership, so the
            // caller keeps its handle and the borrow take covers the call.
            && !self.cur().borrow_params.contains(&reg)
        {
            self.emit(Op::MoveOut { src: reg });
        }
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
        self.compile_resolved_call(dst, c, path, coerce, argc)
    }

    /// `Some(x)`, `Ok(x)`, the other builtin tuple variants, and the empty
    /// container constructors build their value in place, the way a user
    /// variant does, so a `Vec::new()` in a loop skips the path dispatch.
    /// A `with_capacity` argument still runs, its value is only a hint.
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

    /// The path-resolved half of `compile_call`: a known function by id, a
    /// constructor, an associated function, or an external bridge path.
    fn compile_resolved_call(
        &mut self,
        dst: Reg,
        c: &syn::ExprCall,
        path: &syn::Path,
        coerce: Option<TypeIr>,
        argc: u16,
    ) -> Result<()> {
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        let resolved = match self.resolve_path_res(&segs) {
            Ok(r) => r,
            Err(_) => Res::External(segs.clone()),
        };
        let path = match resolved {
            // A known function, called directly by id. Turbofish type args are
            // recorded so the callee can bind them to its generic parameters.
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
            // A tuple struct constructor.
            Res::Struct(canon) => PathRef::user(vec![canon.to_string()], coerce),
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
                PathRef::user(segs, coerce)
            }
            // A tuple struct built through a type alias, `type P = Point; P(..)`.
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
            // Everything else, resolved by the VM through the bridge dispatch.
            Res::External(segs) => {
                let path = self.external_path(segs, coerce);
                match self.compile_external_call(dst, c, path, argc)? {
                    Some(path) => path,
                    None => return Ok(()),
                }
            }
        };
        // Explicit `drop(x)` moves `x` out, so its register clears before
        // the call and the callee sees the last holder, which is what lets
        // a user `Drop` impl run at the call.
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

    /// The bridge paths the compiler lowers in place instead of emitting a
    /// path call. Answers the path back when the call still needs the VM.
    fn compile_external_call(
        &mut self,
        dst: Reg,
        c: &syn::ExprCall,
        path: PathRef,
        argc: u16,
    ) -> Result<Option<PathRef>> {
        // Only `Box::new` is a compile time pass-through: a box is pure
        // ownership, which the value model already gives every value. Rc,
        // Arc, RefCell, Cell, and Mutex build real shared cells at runtime,
        // their sharing is the point of the type.
        if path.id == PathId::BoxNew && c.args.len() == 1 {
            self.compile_into(dst, &c.args[0])?;
            return Ok(None);
        }
        // The mem place functions move whole values between places, which
        // only the compiler can express.
        if matches!(
            path.id,
            PathId::MemSwap | PathId::MemTake | PathId::MemReplace
        ) && self.compile_mem_intrinsic(dst, path.id, c)?
        {
            return Ok(None);
        }
        // Numeric `T::from(x)` lowers to the same cast op as `x as T`. rustc
        // has already proven the conversion lossless, so the widening cast
        // computes the same value without the dynamic path dispatch.
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
        // `v[a..b].copy_from_slice(src)` must write through to `v`. Indexing
        // with a range builds a copied temporary, so the call is compiled
        // against the base vec with the bounds as leading arguments instead.
        // An open end becomes the max sentinel the bridge clamps to the len.
        if m.method == "copy_from_slice" {
            return self.compile_copy_from_slice(dst, m);
        }
        // A zero-argument `take` empties its place: `Option::take` leaves
        // None behind and `RefCell::take` leaves a default. The receiver
        // compiles as a place below because `take` is in the mutating set,
        // and the VM writes the emptied receiver back through it, so the
        // type decides at runtime and `child.stdin.take()` drops the pipe.
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
        // A mutating method's receiver is a place: its storage splits from
        // any value sharing first, and the receiver value stores back after,
        // which lands the split buffer of a string mutation in its place.
        let method_text = m.method.to_string();
        let mutating = BuiltinId::resolve(&method_text).mutates()
            || self.ctx.mut_methods.contains(&method_text);
        let (recv, receiver_place) = if mutating {
            let p = self.compile_mut_receiver(&m.receiver)?;
            (p.reg, Some(p))
        } else {
            (self.compile_expr(&m.receiver)?, None)
        };
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
        if let Some(p) = &receiver_place {
            self.emit_place_writeback(p);
        }
        // Methods that fill a `&mut` argument, like read_line, write the new
        // value into the arg window. The window slot is only a copy of the
        // variable, so move the result back into the variable register.
        self.emit_mut_arg_writebacks(m.args.iter(), base)?;
        Ok(())
    }

    /// `v[a..b].copy_from_slice(src)` must write through to `v`. Indexing
    /// with a range builds a copied temporary, so the call is compiled
    /// against the base vec with the bounds as leading arguments instead.
    /// An open end becomes the max sentinel the bridge clamps to the len.
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
            let env = TyEnv::new(&self.typed_locals, self.ctx.fn_returns);
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
        let env = TyEnv::new(&self.typed_locals, self.ctx.fn_returns);
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
                continue;
            }
            // A register cleared by the borrow take gets its handle back
            // from the callee's returned parameter window. The window slot
            // clears after the move, a stale copy there would inflate
            // `Rc::strong_count` on the next call.
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

    /// The plain immutable local a call argument borrows: `&name`,
    /// `&mut name`, or a plain path forwarding one of this function's own
    /// borrow parameters. A mutable local lives in a cell and a `&mut`
    /// alias points elsewhere, so both stay out.
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

    /// Clear the registers a call borrows, for the call's duration. The
    /// callee then holds the only live handle, so `Rc::strong_count` reads
    /// the same at any call depth, and the writebacks after the call
    /// restore every cleared register from the returned parameter window.
    fn emit_borrow_takes<'e>(&mut self, args: impl Iterator<Item = &'e Expr>) {
        let regs: Vec<Reg> = args.filter_map(|arg| self.borrowed_local(arg)).collect();
        for reg in regs {
            self.emit(Op::LoadUnit { dst: reg });
        }
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
            // An annotated param is a type the program wrote down, recorded
            // like an annotated let, so a default built from the param in the
            // closure body reads the right type.
            if let Pat::Type(t) = p
                && let Pat::Ident(id) = &*t.pat
                && let Some(declared) = annotation_scalar(&t.ty)
            {
                self.typed_locals.insert(id.ident.to_string(), declared);
            }
            // A reference param shares the caller's storage on purpose, so
            // mutable access through it never splits, same rule as fn params.
            if let Pat::Type(t) = p
                && matches!(&*t.ty, syn::Type::Reference(_))
            {
                self.cur().borrow_params.insert(reg);
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
        let mut chunk = child.into_chunk(self.ctx.file.clone())?;
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
                // The base splits from value sharing before the write, so
                // the new element cannot leak into a clone of the container.
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
                // `*r = v` where `r` is a `&mut variable` alias writes the
                // variable itself. The alias may live in this frame or, for
                // a closure that captured it, in an enclosing function's.
                if let Some(name) = place::single_path_name(&u.expr) {
                    let target = match self.unalias(&name) {
                        same if same == name => self.enclosing_alias_target(&name),
                        target => Some(target),
                    };
                    if let Some(target) = target {
                        let location = self.resolve_for_write(&target);
                        let val = self.compile_expr(value)?;
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

    /// `*target op= rhs`, the deref arm of `compile_compound_assign`.
    fn compile_compound_deref_assign(
        &mut self,
        u: &syn::ExprUnary,
        op: BinKind,
        rhs: &Expr,
    ) -> Result<()> {
        // `*r op= rhs` where `r` is a `&mut variable` alias reads and writes
        // the variable itself.
        if let Some(name) = place::single_path_name(&u.expr) {
            let target = match self.unalias(&name) {
                // A closure dereferencing an alias it captured finds the
                // alias in an enclosing function's frame.
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
            // Not a `&mut` parameter: the fused op holds the referent's
            // lock across a scalar read-modify-write, so concurrent
            // compound assignments through a shared cell cannot lose
            // updates.
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
        let lowered = self.lower_pattern(pat);
        let f = self.cur();
        f.pats.push(PatInfo {
            pat: lowered,
            binds,
        });
        Ok(u16::try_from(f.pats.len() - 1)?)
    }

    /// The variant a pattern path names, through the user enums first and
    /// the builtin tables second. A path nothing resolves keeps only its
    /// last segment, and the runtime test falls back to the name.
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

/// The cast target of a numeric `T::from(x)` call, when the whole call is
/// exactly that shape.
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

/// The empty container a constructor path builds, lowered to one op so a
/// `Vec::new()` in a loop does not go through the path dispatch.
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
