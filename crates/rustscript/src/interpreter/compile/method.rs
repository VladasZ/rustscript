//! Method calls, the `collect`, `sum` and `unwrap_or_default` type hints they read, and fold closures.

use anyhow::{Result, bail};
use syn::Expr;

use crate::interpreter::bytecode::{BinKind, BuiltinId, DISCARD, Op, PathRef, Reg, ScalarTy};

use super::infer::Ty;
use super::place;
use super::walks::unparen;
use super::{CollectTarget, Compiler, NameLoc, idx16};

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

    pub(super) fn compile_method(&mut self, dst: Reg, m: &syn::ExprMethodCall) -> Result<()> {
        if m.method == "copy_from_slice" {
            return self.compile_copy_from_slice(dst, m);
        }
        if self.compile_into_iter(dst, m)? {
            return Ok(());
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
        // `v.into()` into a script type is `T::from(v)`, anything else is identity
        if m.method == "into"
            && m.args.is_empty()
            && let Ty::Struct(canon) | Ty::Enum(canon) = self.types.of_node(m)
        {
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
            return Ok(());
        }
        let method_text = m.method.to_string();
        let mutating = (BuiltinId::resolve(&method_text).mutates()
            || self.ctx.mut_methods.contains(&method_text))
            // `rotate_left` mutates a slice but returns a value on an integer, writing back over
            // an integer receiver would undo the assignment
            && !(matches!(method_text.as_str(), "rotate_left" | "rotate_right")
                && matches!(self.types.of(&m.receiver), Ty::Int(_)));
        let owned = self.scrutinee_owned(&m.receiver);
        let (recv, receiver_place) = if mutating {
            let p = self.compile_mut_receiver(&m.receiver)?;
            (p.reg, Some(p))
        } else if consumes_receiver(&method_text) && !self.lends_receiver(m, &method_text) {
            // a method that takes `self` owns the receiver now, so a local moves in and a
            // temporary is not dropped again at the statement end
            (self.compile_owned_expr(&m.receiver)?, None)
        } else {
            (self.compile_expr(&m.receiver)?, None)
        };
        let place = mutating && place::is_place_expr(&m.receiver);
        let base = self.compile_shared_args(m.args.iter())?;
        let (method, scalar) = self.method_name_and_scalar(m);
        let default = if method == "unwrap_or_default" {
            let ty = self.types.of_node(m);
            self.default_ir_of(&ty)
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

    /// `collect` into a String renames to `collect_string`, a map to `collect_map`, a set to
    /// `collect_set`, from the turbofish or the inferred result. The scalar a method needs at
    /// runtime rides on the name, see `method_scalar`.
    pub(super) fn method_name_and_scalar(
        &mut self,
        m: &syn::ExprMethodCall,
    ) -> (String, Option<ScalarTy>) {
        let mut method = m.method.to_string();
        if method == "collect" {
            let target = m
                .turbofish
                .as_ref()
                .and_then(turbofish_collect_target)
                .or_else(|| self.collect_target_of(m));
            if let Some(target) = target {
                method = target.method_name().to_string();
            }
        }
        let scalar =
            turbofish_scalar(m.turbofish.as_ref()).or_else(|| self.method_scalar(m, &method));
        (method, scalar)
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
            | "any"
            | "all"
            | "position"
            | "rposition"
            | "find"
            | "find_map"
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
