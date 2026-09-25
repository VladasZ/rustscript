//! Method calls, the `collect`, `sum` and `unwrap_or_default` type hints they read, and fold closures.

use anyhow::{Result, bail};
use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;
use syn::Expr;

use crate::interpreter::bytecode::{BinKind, DISCARD, Op, PathRef, Reg, ScalarTy};

use super::infer::Ty;
use super::place;
use super::walks::unparen;
use super::{CollectInner, CollectTarget, Compiler, NameLoc, idx16};
use crate::interpreter::bytecode::DefaultIr;

impl Compiler<'_> {
    /// `last` is the consuming terminal on an iterator and the slice method on a collection,
    /// which lends, so a collection temporary still drops at the statement end.
    fn lends_receiver(&self, m: &syn::ExprMethodCall, name: &str) -> bool {
        name == "last"
            && matches!(
                self.types.of(&m.receiver),
                Ty::Vec(_) | Ty::Str | Ty::Tuple(_)
            )
    }

    /// Whether a by value `self` call owns its receiver. A local that is not a borrow, a place
    /// rooted in one, or a temporary. A borrow parameter forwards a handle it does not own.
    fn consumes_receiver(&mut self, expr: &Expr) -> bool {
        match unparen(expr) {
            Expr::Path(p) if p.path.segments.len() == 1 && p.qself.is_none() => {
                let name = p.path.segments[0].ident.to_string();
                if self.cur().aliases.contains_key(&name) {
                    return false;
                }
                match self.resolve(&name) {
                    NameLoc::Local(reg) => !self.cur().shares_only(reg),
                    NameLoc::Cell(_) => true,
                    NameLoc::Upvalue(_) | NameLoc::None => false,
                }
            }
            Expr::Field(_) | Expr::Index(_) => self.place_root(expr).is_some(),
            Expr::Call(_) | Expr::Macro(_) | Expr::Struct(_) | Expr::Array(_) | Expr::Tuple(_) => {
                true
            }
            _ => false,
        }
    }

    /// `v.into_iter()` consumes `v`, so the iterator takes the items like a `for` over `v`. A
    /// borrow parameter or a `&v` only lends them, and a receiver the compiler can not place,
    /// the result of a lending method, goes through the native and lends too.
    fn compile_into_iter(&mut self, dst: Reg, m: &syn::ExprMethodCall) -> Result<bool> {
        if m.method != "into_iter" || !m.args.is_empty() || m.turbofish.is_some() {
            return Ok(false);
        }
        let receiver = unparen(&m.receiver);
        let owned = match receiver {
            Expr::Path(_) | Expr::Field(_) | Expr::Index(_) => self.consumes_receiver(receiver),
            Expr::Reference(_) => false,
            other => self.temp_owned(other),
        };
        if !owned && !matches!(receiver, Expr::Path(_) | Expr::Reference(_)) {
            return Ok(false);
        }
        let src = if owned {
            self.compile_owned_expr(receiver)?
        } else {
            self.compile_expr(receiver)?
        };
        self.emit(Op::IterInit { dst, src, owned });
        Ok(true)
    }

    /// `map.range(a..b)` on a `BTreeMap` or `BTreeSet`. A range value keeps an open start as 0,
    /// which is wrong for negative or string keys, so each bound goes as its own argument with a
    /// flag for whether it is there. The args are start, has start, end, has end, inclusive.
    fn compile_sorted_range(&mut self, dst: Reg, m: &syn::ExprMethodCall) -> Result<bool> {
        if m.method != "range" || m.args.len() != 1 {
            return Ok(false);
        }
        if !matches!(
            self.types.of(&m.receiver),
            Ty::Map(_, _, true) | Ty::Set(_, true)
        ) {
            return Ok(false);
        }
        let Expr::Range(r) = unparen(&m.args[0]) else {
            return Ok(false);
        };
        let unit: Expr = syn::parse_quote!(());
        let flag = |b: bool| -> Expr { syn::parse_quote!(#b) };
        let args = [
            r.start.as_deref().cloned().unwrap_or_else(|| unit.clone()),
            flag(r.start.is_some()),
            r.end.as_deref().cloned().unwrap_or_else(|| unit.clone()),
            flag(r.end.is_some()),
            flag(matches!(r.limits, syn::RangeLimits::Closed(_))),
        ];
        let recv = self.compile_expr(&m.receiver)?;
        let base = self.compile_shared_args(args.iter())?;
        let name = self.add_name_full("btree_range".to_string(), None, None, false, false);
        self.set_line(m.method.span());
        self.emit(Op::Method {
            dst,
            recv,
            name,
            base,
            argc: idx16(args.len()),
        });
        Ok(true)
    }

    /// `zip` and `chain` take their iterable by value, a fresh collection hands its items to
    /// the adapter, which drops the ones nobody pulled.
    /// The argument compiles as written, a quoted `(arg).into_iter()` would be a node the type
    /// table does not know.
    fn compile_method_args(&mut self, m: &syn::ExprMethodCall, method: &str) -> Result<Reg> {
        match m.args.first() {
            Some(arg)
                if self.ctx.has_drop
                    && m.args.len() == 1
                    && matches!(method, "zip" | "chain")
                    && !matches!(unparen(arg), Expr::Reference(_))
                    && !matches!(self.types.of(&m.receiver), Ty::Option(_))
                    && !matches!(self.types.of(arg), Ty::Option(_))
                    && self.temp_owned(arg) =>
            {
                let base = self.alloc();
                let src = self.compile_owned_expr(arg)?;
                self.emit(Op::IterInit {
                    dst: base,
                    src,
                    owned: true,
                });
                Ok(base)
            }
            _ => self.compile_shared_args(m.args.iter()),
        }
    }

    /// Strict inference. A method whose result type comes from the context must know that
    /// type, a guess could print a wrong result where compiled Rust is right.
    fn check_target_known(&self, m: &syn::ExprMethodCall) -> Result<()> {
        if !m.args.is_empty() {
            return Ok(());
        }
        let method = m.method.to_string();
        let ty = self.types.of_node(m);
        let unknown = match method.as_str() {
            "collect" => {
                ty.is_unknown() && m.turbofish.is_none() && self.collect_target(m).is_none()
            }
            "parse" => m.turbofish.is_none() && ty.payload().is_unknown(),
            "or_default" => matches!(self.types.of(&m.receiver), Ty::Entry(_)) && ty.is_unknown(),
            "into" => {
                self.types.is_unresolved(m) && self.user_from_accepts(&self.types.of(&m.receiver))
            }
            _ => false,
        };
        if unknown {
            let line = m.method.span().start().line;
            bail!(
                "unsupported: line {line} of {}, the interpreter cannot tell what type `{method}` \
                 makes here, name it with a turbofish or a `let` annotation",
                self.ctx.file
            );
        }
        Ok(())
    }

    pub(super) fn compile_method(&mut self, dst: Reg, m: &syn::ExprMethodCall) -> Result<()> {
        self.check_target_known(m)?;
        if m.method == "copy_from_slice" {
            return self.compile_copy_from_slice(dst, m);
        }
        if self.compile_sorted_range(dst, m)? {
            return Ok(());
        }
        if self.compile_into_iter(dst, m)? {
            return Ok(());
        }
        // `x.get(k).copied().unwrap_or(d)` builds and tears down an Option per call, that
        // dominates counting loops. The fused op builds `d` before the clone, so with a `Drop`
        // impl a panic in `d` would miss the clone it has to drop.
        if dst != DISCARD
            && m.method == "unwrap_or"
            && m.args.len() == 1
            && let Expr::MethodCall(c) = &*m.receiver
            && (c.method == "copied" || (c.method == "cloned" && !self.ctx.has_drop))
            && c.args.is_empty()
            && let Expr::MethodCall(g) = &*c.receiver
            && g.method == "get"
            && g.args.len() == 1
        {
            let recv = self.compile_expr(&g.receiver)?;
            let key = self.compile_expr(&g.args[0])?;
            // the default moves into the result, a shared handle would be dropped twice
            let default = self.compile_owned_expr(&m.args[0])?;
            self.emit(Op::GetOrDefault {
                dst,
                recv,
                key,
                default,
            });
            return Ok(());
        }
        if self.compile_into_conversion(dst, m)? || self.compile_parse_user(dst, m)? {
            return Ok(());
        }
        if self.compile_json_to_string(dst, m)? {
            return Ok(());
        }
        let method_text = m.method.to_string();
        // writing back over an integer receiver of `rotate_left` would undo the assignment
        let mutating = self.method_mutates(m);
        let owned = self.scrutinee_owned(&m.receiver);
        let mut unwinds_receiver = false;
        // the tail call of a block unwinds its receiver like the temporaries around it, in
        // reverse order of creation, any other call unwinds it first
        let tail = std::mem::take(&mut self.cur().tail_call);
        let held = self.cur().unwind_temps.len();
        let (recv, receiver_place) = if mutating {
            let p = self.compile_mut_receiver(&m.receiver)?;
            (p.reg, Some(p))
        } else if consumes_receiver(&method_text) && !self.lends_receiver(m, &method_text) {
            // a method that takes `self` owns the receiver now, so a local moves in and a
            // temporary is not dropped again at the statement end
            let reg = self.compile_owned_expr(&m.receiver)?;
            // until the call runs, a panic in an argument is the only thing that drops it
            if self.ctx.has_drop && self.arg_owned(&m.receiver) {
                if tail {
                    self.cur().owned_temps.push(reg);
                } else {
                    self.cur().hold_operand(reg);
                }
                unwinds_receiver = true;
            }
            (reg, None)
        } else {
            let reg = self.compile_expr(&m.receiver)?;
            (self.read_before_args(reg, m), None)
        };
        let place = mutating && place::is_place_expr(&m.receiver);
        let base = self.compile_method_args(m, &method_text)?;
        self.cur().close_operands(held);
        let (method, scalar) = self.method_name_and_scalar(m);
        let default = if matches!(method.as_str(), "unwrap_or_default" | "or_default") {
            let ty = self.types.of_node(m);
            self.default_ir_of(&ty)
        } else if matches!(method.as_str(), "collect_result" | "collect_option") {
            self.collect_inner_default(m)
        } else {
            None
        };
        let name = self.add_name_full(method, scalar, default, place, owned);
        // restamp with the method's own line, the one `rustc` names for a multiline chain
        self.set_line(m.method.span());
        self.emit(Op::Method {
            dst,
            recv,
            name,
            base,
            argc: idx16(m.args.len()),
        });
        // the call took it, so a later panic in this frame must not drop it again
        if unwinds_receiver {
            self.emit(Op::LoadUnit { dst: recv });
        }
        if let Some(p) = &receiver_place {
            // `opt.take()` on a captured variable hands the payload out, so the store back
            // must not drop the old value as overwritten, see `emit_place_take`
            if matches!(method_text.as_str(), "take" | "replace")
                && matches!(self.types.of(&m.receiver), Ty::Option(_))
            {
                self.emit_place_take(p);
            }
            self.emit_place_writeback(p);
        }
        // `read_line` and friends write into the arg window copy, so move the result back into
        // the variable
        self.emit_mut_arg_writebacks(m.args.iter(), base)?;
        Ok(())
    }

    /// `to_string` on a `serde_json::Value`. A json value is a plain map, list or string at
    /// runtime, only the type says it prints as json.
    fn compile_json_to_string(&mut self, dst: Reg, m: &syn::ExprMethodCall) -> Result<bool> {
        if m.method != "to_string" || !m.args.is_empty() || self.types.of(&m.receiver) != Ty::Json {
            return Ok(false);
        }
        let base = self.compile_args(std::iter::once(&*m.receiver))?;
        let path = self.add_path(PathRef::new(vec!["::json_to_string".to_string()], None));
        self.emit(Op::CallPath {
            dst,
            path,
            base,
            argc: 1,
        });
        Ok(true)
    }

    /// `v.into()` into a script type is `T::from(v)`, anything else is identity and stays a
    /// method call. True when the conversion was emitted here.
    fn compile_into_conversion(&mut self, dst: Reg, m: &syn::ExprMethodCall) -> Result<bool> {
        if m.method != "into" || !m.args.is_empty() {
            return Ok(false);
        }
        let (Ty::Struct(canon) | Ty::Enum(canon)) = self.types.of_node(m) else {
            return Ok(false);
        };
        let source = self.types.of(&m.receiver);
        let path = PathRef::user(self.impl_path_for_from(&canon, &source), None);
        let p = self.add_path(path);
        let base = self.compile_args(std::iter::once(&*m.receiver))?;
        self.emit(Op::CallPath {
            dst,
            path: p,
            base,
            argc: 1,
        });
        Ok(true)
    }

    /// `s.parse::<T>()` into a script type is `T::from_str(s)`, which is what std calls. True
    /// when the call was emitted here.
    fn compile_parse_user(&mut self, dst: Reg, m: &syn::ExprMethodCall) -> Result<bool> {
        if m.method != "parse" || !m.args.is_empty() {
            return Ok(false);
        }
        let (Ty::Struct(canon) | Ty::Enum(canon)) = self.types.of_node(m).payload() else {
            return Ok(false);
        };
        if !self
            .ctx
            .impl_sigs
            .contains_key(&(canon.to_string(), "from_str".to_string()))
        {
            return Ok(false);
        }
        let path = PathRef::user(vec![canon.to_string(), "from_str".to_string()], None);
        let p = self.add_path(path);
        let base = self.compile_args(std::iter::once(&*m.receiver))?;
        self.set_line(m.method.span());
        self.emit(Op::CallPath {
            dst,
            path: p,
            base,
            argc: 1,
        });
        Ok(true)
    }

    /// `v.rem_euclid(replace(&mut v, 1))` reads `v` before the argument writes it. A scalar
    /// local is its register, so an argument that names it gets a copy to work against.
    fn read_before_args(&mut self, recv: Reg, m: &syn::ExprMethodCall) -> Reg {
        self.read_before_writes(recv, &m.receiver, m.args.iter())
    }

    /// `v / replace(&mut v, 2)` reads the left operand before the right one runs, like the
    /// receiver above. `reg` is the compiled `expr`, a copy when a later expression names it.
    pub(super) fn read_before_writes<'e>(
        &mut self,
        reg: Reg,
        expr: &Expr,
        later: impl Iterator<Item = &'e Expr>,
    ) -> Reg {
        let Some(name) = place::single_path_name(expr) else {
            return reg;
        };
        let scalar = matches!(
            self.types.of(expr),
            Ty::Int(_) | Ty::IntVar(_) | Ty::F32 | Ty::F64 | Ty::FloatVar(_) | Ty::Bool | Ty::Char
        );
        let mut later = later;
        if !scalar || !later.any(|e| mentions_ident(e.to_token_stream(), &name)) {
            return reg;
        }
        let copy = self.alloc();
        self.emit(Op::Copy {
            dst: copy,
            src: reg,
        });
        copy
    }

    /// `v[a..b].copy_from_slice(src)` must write through to `v`, and a range index builds a copy. So
    /// the call compiles against the base vec with the bounds as leading arguments.
    pub(super) fn compile_copy_from_slice(
        &mut self,
        dst: Reg,
        m: &syn::ExprMethodCall,
    ) -> Result<()> {
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

    /// `collect` into a String renames to `collect_string`, a map to `collect_map`, a set to
    /// `collect_set`, from the turbofish or the inferred result. The scalar a method needs at
    /// runtime rides on the name, see `method_scalar`.
    pub(super) fn method_name_and_scalar(
        &mut self,
        m: &syn::ExprMethodCall,
    ) -> (String, Option<ScalarTy>) {
        let mut method = m.method.to_string();
        if method == "collect"
            && let Some(target) = self.collect_target(m)
        {
            method = target.method_name().to_string();
        }
        let scalar =
            turbofish_scalar(m.turbofish.as_ref()).or_else(|| self.method_scalar(m, &method));
        (method, scalar)
    }
}

impl Compiler<'_> {
    /// The written turbofish first, then the inferred result. A turbofish `Result<_, _>` takes
    /// its `C` from the inferred type.
    pub(super) fn collect_target(&self, m: &syn::ExprMethodCall) -> Option<CollectTarget> {
        let inferred = self.collect_target_of(m);
        let written = m.turbofish.as_ref().and_then(turbofish_collect_target);
        match (written, inferred) {
            (Some(CollectTarget::Result(None)), Some(CollectTarget::Result(inner)))
            | (Some(CollectTarget::Option(None)), Some(CollectTarget::Option(inner))) => written
                .map(|w| match w {
                    CollectTarget::Result(_) => CollectTarget::Result(inner),
                    _ => CollectTarget::Option(inner),
                }),
            (Some(w), _) => Some(w),
            (None, i) => i,
        }
    }

    /// The empty `C` of a collect into `Result<C, E>` or `Option<C>`, a `Vec` when nothing
    /// says otherwise.
    pub(super) fn collect_inner_default(&self, m: &syn::ExprMethodCall) -> Option<DefaultIr> {
        match self.collect_target(m)? {
            CollectTarget::Result(inner) | CollectTarget::Option(inner) => {
                Some(inner.unwrap_or(CollectInner::Vec).empty())
            }
            _ => None,
        }
    }
}

pub(super) fn turbofish_collect_target(
    tf: &syn::AngleBracketedGenericArguments,
) -> Option<CollectTarget> {
    tf.args.iter().find_map(|arg| match arg {
        syn::GenericArgument::Type(ty) => CollectTarget::of_type(ty),
        _ => None,
    })
}

pub(super) fn turbofish_scalar(
    args: Option<&syn::AngleBracketedGenericArguments>,
) -> Option<ScalarTy> {
    args?
        .args
        .iter()
        .find_map(|arg| match arg {
            syn::GenericArgument::Type(ty) => Some(ty),
            _ => None,
        })
        .and_then(ScalarTy::lower)
}

/// Methods that take `self` by value. The receiver temporary is theirs, so its drop, if any, is
/// on them and not on the statement end. A borrowing method leaves the temporary behind to
/// drop at the semicolon.
pub(super) fn consumes_receiver(name: &str) -> bool {
    matches!(
        name,
        "unwrap"
            | "expect"
            | "unwrap_or"
            | "unwrap_or_else"
            | "unwrap_or_default"
            | "unwrap_err"
            | "expect_err"
            | "ok"
            | "err"
            | "ok_or"
            | "ok_or_else"
            | "map"
            | "map_err"
            | "map_or"
            | "map_or_else"
            | "and_then"
            | "and"
            | "or"
            | "or_else"
            | "xor"
            | "zip"
            | "flatten"
            | "filter"
            | "into"
            | "into_iter"
            | "into_keys"
            | "into_values"
            | "into_inner"
            | "into_boxed_slice"
            | "into_bytes"
            | "into_string"
            | "collect"
            | "sum"
            | "product"
            | "count"
            | "fold"
            | "reduce"
            | "min"
            | "max"
            | "min_by"
            | "max_by"
            | "min_by_key"
            | "max_by_key"
            | "last"
            | "for_each"
            | "rev"
            | "take"
            | "skip"
            | "step_by"
            | "enumerate"
            | "chain"
            | "flat_map"
            | "filter_map"
            | "take_while"
            | "skip_while"
            | "map_while"
            | "scan"
            | "inspect"
            | "peekable"
            | "cycle"
            | "fuse"
            | "unzip"
            | "partition"
            | "cloned"
            | "copied"
            | "then"
            | "then_some"
            | "is_some_and"
            | "is_none_or"
            | "is_ok_and"
            | "is_err_and"
            | "transpose"
            | "unwrap_unchecked"
    )
}

/// Whether the tokens name the identifier anywhere, inside nested groups too.
fn mentions_ident(tokens: TokenStream, name: &str) -> bool {
    tokens.into_iter().any(|tree| match tree {
        TokenTree::Ident(ident) => ident == name,
        TokenTree::Group(group) => mentions_ident(group.stream(), name),
        _ => false,
    })
}
