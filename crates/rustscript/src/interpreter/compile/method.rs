//! Method calls, the `collect`, `sum` and `unwrap_or_default` type hints they read, and fold closures.

use anyhow::{Result, bail};
use syn::{Expr, Pat};

use crate::interpreter::bytecode::{
    BinKind, BuiltinId, DISCARD, DefaultIr, Op, PathRef, Reg, ScalarTy,
};
use crate::interpreter::numeric::IntWidth;

use super::place;
use super::walks::{tail_exprs, takes_numeric_hint, unparen};
use super::written::{TyEnv, element_of, option_payload, turbofish_scalar, written_ty};
use super::{CollectTarget, Compiler, NumericTy, idx16, numeric_target};

impl Compiler<'_> {
    pub(super) fn compile_method(&mut self, dst: Reg, m: &syn::ExprMethodCall) -> Result<()> {
        self.seed_bare_receiver_width(m);
        self.seed_bare_fallback_width(m);
        if m.method == "copy_from_slice" {
            return self.compile_copy_from_slice(dst, m);
        }
        // `x.get(k).copied().unwrap_or(d)` builds and tears down an Option per call, that
        // dominates counting loops
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
        // an `unwrap_or_default` unwrapped again must have produced an `Option`, so its default
        // is `None`
        let outer_option_hint = self.option_result.take();
        if m.method == "unwrap_or_default"
            && let Expr::MethodCall(inner) = &*m.receiver
            && inner.method == "unwrap_or_default"
        {
            self.option_result = Some(std::ptr::from_ref(inner));
        }
        // `v.into()` under `let x: T` is `T::from(v)`, without one it is identity
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
        // `Sum<T> for T` types the body of a `map` closure as `T`, so its literals adopt the
        // width of the reduction
        if matches!(method_text.as_str(), "sum" | "product")
            && let Some(target) = self.reduce_target(m)
        {
            self.seed_reduce_closure(&m.receiver, target);
        }
        let mutating = (BuiltinId::resolve(&method_text).mutates()
            || self.ctx.mut_methods.contains(&method_text))
            // `rotate_left` mutates a slice but returns a value on an integer, writing back over
            // an integer receiver would undo the assignment
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
        // The accumulator of a `fold` closure is the init's type and the item is the element, so
        // a default built inside the body knows its type.
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
        // restamp with the method's own line, the one `rustc` names for a multiline chain
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
        // `read_line` and friends write into the arg window copy, so move the result back into
        // the variable
        self.emit_mut_arg_writebacks(m.args.iter(), base)?;
        Ok(())
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

    /// `collect` into a String renames to `collect_string` from a turbofish, a pending `let s: String`,
    /// or a `-> String` signature. See `Compiler::string_let` and `Compiler::string_tails`.
    pub(super) fn method_name_and_scalar(
        &mut self,
        m: &syn::ExprMethodCall,
    ) -> (String, Option<ScalarTy>) {
        let mut method = m.method.to_string();
        if method == "collect" {
            let from_turbofish = m.turbofish.as_ref().and_then(turbofish_collect_target);
            let from_let = match self.collect_let {
                Some((ptr, target)) if std::ptr::eq(ptr, m) => Some(target),
                _ => None,
            };
            let from_tail = self.collect_tails.get(&std::ptr::from_ref(m)).copied();
            if let Some(target) = from_turbofish.or(from_let).or(from_tail) {
                // Cleared only when this call consumed the hint. A nested turbofish collect must not
                // clear it, otherwise the outer collect falls back to a vec of pairs.
                if from_let.is_some() {
                    self.collect_let = None;
                }
                method = target.method_name().to_string();
            }
        }
        // the turbofish rides on the name for the methods that need it
        let mut scalar = turbofish_scalar(m.turbofish.as_ref());
        // `unwrap_or_default` takes its type from the receiver payload, as `None::<u64>` or
        // `then_some(1u8)` state it
        if scalar.is_none() && m.method == "unwrap_or_default" {
            let env = TyEnv::new(&self.typed_locals, self.ctx.fn_returns);
            scalar = option_payload(&m.receiver, &env);
        }
        // an empty vec has no shape to dispatch a script method on, so the written type rides along
        if scalar.is_none() && self.ctx.method_atoms.contains_key(&method) {
            scalar = self.stated_ty(&m.receiver);
        }
        // `concat` of nothing can't tell nested vecs from strings
        if scalar.is_none() && method == "concat" {
            let env = TyEnv::new(&self.typed_locals, self.ctx.fn_returns);
            scalar = element_of(&m.receiver, &env);
        }
        // `let x: T = ...sum()` names the width of the outermost reduction
        if scalar.is_none()
            && (method == "sum" || method == "product")
            && let Some((ptr, ty)) = &self.reduce_let
            && std::ptr::eq(*ptr, m)
        {
            scalar = Some(ty.clone());
            self.reduce_let = None;
        }
        // `let x: T = ...unwrap_or_default()` names the outermost payload
        if scalar.is_none()
            && let Some((ptr, ty)) = &self.default_let
            && std::ptr::eq(*ptr, m)
        {
            scalar = Some(ty.clone());
            self.default_let = None;
        }
        // a `-> T` signature names a bare reduction or default handed back
        if scalar.is_none()
            && matches!(method.as_str(), "sum" | "product" | "unwrap_or_default")
            && let Some(ty) = self.return_tails.get(&std::ptr::from_ref(m))
        {
            scalar = ScalarTy::lower(ty);
        }
        // failing all that, a result unwrapped again is an Option
        if matches!(self.option_result, Some(ptr) if std::ptr::eq(ptr, m)) {
            self.option_result = None;
            scalar = scalar.or(Some(ScalarTy::Opt(Box::new(ScalarTy::Other))));
        }
        (method, scalar)
    }

    /// Read without consuming the hint.
    pub(super) fn reduce_target(&self, m: &syn::ExprMethodCall) -> Option<NumericTy> {
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

    /// A receiver of only unsuffixed literals is `i32`. Its width picks the method and the `From`
    /// impl, so it is settled first.
    pub(super) fn seed_bare_receiver_width(&mut self, m: &syn::ExprMethodCall) {
        if self
            .numeric_hints
            .contains_key(&std::ptr::from_ref(&*m.receiver))
            || !super::walks::bare_int_rooted(&m.receiver)
        {
            return;
        }
        self.numeric_hints.insert(
            std::ptr::from_ref(&*m.receiver),
            NumericTy::Int(IntWidth::I32),
        );
    }

    /// `unwrap_or(v)` gives the fallback the payload type. Without one it is `i32`.
    pub(super) fn seed_bare_fallback_width(&mut self, m: &syn::ExprMethodCall) {
        if m.method != "unwrap_or" {
            return;
        }
        let Some(arg) = m.args.first() else {
            return;
        };
        if self.numeric_hints.contains_key(&std::ptr::from_ref(arg))
            || !super::walks::bare_int_rooted(arg)
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
    pub(super) fn bind_fold_params(
        &mut self,
        m: &syn::ExprMethodCall,
    ) -> Vec<(String, Option<syn::Type>)> {
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
        // the body produces the next accumulator, so it has the init's width
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

    pub(super) fn seed_reduce_closure(&mut self, expr: &Expr, target: NumericTy) {
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
    pub(super) fn default_for_unwrap(&mut self, m: &syn::ExprMethodCall) -> Option<DefaultIr> {
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
}

pub(super) fn turbofish_collect_target(
    tf: &syn::AngleBracketedGenericArguments,
) -> Option<CollectTarget> {
    tf.args.iter().find_map(|arg| match arg {
        syn::GenericArgument::Type(ty) => CollectTarget::of_type(ty),
        _ => None,
    })
}
