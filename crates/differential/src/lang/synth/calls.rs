//! Calls of user items, closures, helpers, script functions and trait methods.

use rand::RngExt;

use crate::lang::block::{Param, ParamMode};
use crate::lang::expr::{Expr, unbare_deep};
use crate::lang::synth::{BindKind, Generator, MAX_EXPR_DEPTH};
use crate::lang::ty::Ty;
use crate::lang::user::{MethodKind, UserShape};

impl Generator<'_> {
    // user types

    pub(super) fn user_expr(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let method = self.method_call(want, depth);
        if method.is_some() && self.chance(0.5) {
            return method;
        }
        let Ty::User(shape) = want else {
            if want.has_default() && self.chance(0.3) {
                return Some(Expr::DefaultOf(want.clone()));
            }
            return method;
        };
        let shape = shape.clone();
        match self.rng.random_range(0..6) {
            0 if shape.derives.default => Some(Expr::DefaultOf(want.clone())),
            1 if !shape.froms.is_empty() => {
                let src = self.pick(&shape.froms).clone();
                let value = unbare_deep(self.typed_only(|inner| inner.expr(&src, depth - 1)));
                Some(Expr::Into {
                    value: Box::new(value),
                    to: want.clone(),
                    bare: false,
                })
            }
            2 => self.assoc_call(&shape, depth),
            _ => Some(self.user_literal(&shape, depth - 1)),
        }
    }

    pub(super) fn user_literal(&mut self, shape: &UserShape, depth: usize) -> Expr {
        if shape.is_enum() {
            let variant = self.rng.random_range(0..shape.variants().len());
            let payload = shape.variants()[variant]
                .payload
                .iter()
                .map(|ty| self.expr(ty, depth))
                .collect();
            return Expr::EnumLit {
                shape: Box::new(shape.clone()),
                variant,
                payload,
            };
        }
        let fields = shape.fields();
        let update = shape.derives.default && fields.len() > 1 && self.chance(0.4);
        let written = if update {
            self.rng.random_range(0..fields.len())
        } else {
            fields.len()
        };
        let values = fields[..written]
            .iter()
            .map(|field| self.expr(&field.ty, depth))
            .collect();
        Expr::StructLit {
            shape: Box::new(shape.clone()),
            fields: values,
            update,
        }
    }

    pub(super) fn assoc_call(&mut self, shape: &UserShape, depth: usize) -> Option<Expr> {
        let assoc: Vec<_> = shape
            .methods
            .iter()
            .filter(|sig| sig.kind == MethodKind::Assoc)
            .cloned()
            .collect();
        if assoc.is_empty() {
            return None;
        }
        let sig = self.pick(&assoc).clone();
        let args = sig.args.iter().map(|ty| self.expr(ty, depth - 1)).collect();
        Some(Expr::Method {
            owner: Box::new(shape.clone()),
            name: sig.name,
            kind: MethodKind::Assoc,
            base: None,
            args,
            ty: Ty::user(shape.clone()),
        })
    }

    pub(super) fn method_call(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let mut options: Vec<(UserShape, crate::lang::user::MethodSig)> = Vec::new();
        for def in &self.types {
            let owner = def.ty();
            for sig in &def.shape.methods {
                if sig.kind == MethodKind::Method && sig.ret_ty(&owner) == *want {
                    options.push((def.shape.clone(), sig.clone()));
                }
            }
        }
        if options.is_empty() {
            return None;
        }
        let (shape, sig) = self.pick(&options).clone();
        let owner = Ty::user(shape.clone());
        let base = self.expr(&owner, depth - 1);
        let args = sig.args.iter().map(|ty| self.expr(ty, depth - 1)).collect();
        Some(Expr::Method {
            owner: Box::new(shape),
            name: sig.name,
            kind: MethodKind::Method,
            base: Some(Box::new(base)),
            args,
            ty: want.clone(),
        })
    }

    // named calls

    pub(super) fn call_named(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let mut options: Vec<Expr> = Vec::new();
        if let Some(expr) = self.closure_call(want, depth) {
            options.push(expr);
        }
        if let Some(expr) = self.helper_call(want, depth) {
            options.push(expr);
        }
        if *want == Ty::Str
            && let Some(expr) = self.trait_call(depth)
        {
            options.push(expr);
        }
        if self.chance(0.15) {
            let name = self.generic_pick_fn();
            let first = self.expr(want, depth - 1);
            let second = self.expr(want, depth - 1);
            let flag = self.expr(&Ty::Bool, depth - 1);
            options.push(Expr::FnCall {
                name,
                args: vec![first, second, flag],
                by_ref: Vec::new(),
                ty: want.clone(),
            });
        }
        if options.is_empty() {
            return None;
        }
        Some(self.pick(&options).clone())
    }

    pub(super) fn closure_call(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let closures: Vec<(String, Vec<Ty>)> = self
            .scope
            .iter()
            .filter_map(|binding| match &binding.kind {
                BindKind::Closure { params, ret } if ret == want => {
                    Some((binding.name.clone(), params.clone()))
                }
                _ => None,
            })
            .filter(|(name, _)| !self.called_closures.contains(name))
            .collect();
        if closures.is_empty() {
            return None;
        }
        let (name, params) = self.pick(&closures).clone();
        self.called_closures.push(name.clone());
        // a closure over its own return type can also go through the apply helper
        if params.len() == 1 && params[0] == *want && self.chance(0.3) {
            let helper = self.apply_fn(want);
            // the helper holds the closure by `&mut`, so the argument must not call it
            let arg = self.without_binding(&name, |inner| inner.expr(want, depth - 1));
            return Some(Expr::ApplyCall {
                helper,
                closure: name,
                arg: Box::new(arg),
                ty: want.clone(),
            });
        }
        // a `FnMut` borrow is exclusive, so the arguments must not reach the same closure
        let args = self.without_binding(&name, |inner| {
            params
                .iter()
                .map(|ty| inner.expr(ty, depth - 1))
                .collect::<Vec<_>>()
        });
        Some(Expr::ClosureCall {
            name,
            args,
            ty: want.clone(),
        })
    }

    pub(super) fn helper_call(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let plain: Vec<(String, Vec<Param>)> = self
            .fns
            .iter()
            .filter_map(|def| match &def.kind {
                crate::lang::block::FnKind::Plain { params, ret, .. } if ret == want => {
                    Some((def.name.clone(), params.clone()))
                }
                _ => None,
            })
            .collect();
        let (name, params) = if plain.is_empty() || self.chance(0.3) {
            // a new helper, so the wanted type gets a body out of sight
            if depth < MAX_EXPR_DEPTH {
                return None;
            }
            self.helper_fn(want)?
        } else {
            self.pick(&plain).clone()
        };
        Some(self.fn_call(name, &params, want, depth - 1))
    }

    pub(super) fn fn_call(
        &mut self,
        name: String,
        params: &[Param],
        ret: &Ty,
        depth: usize,
    ) -> Expr {
        let args = params
            .iter()
            .map(|param| self.expr(&param.ty, depth))
            .collect();
        Expr::FnCall {
            name,
            args,
            by_ref: params
                .iter()
                .map(|param| param.mode == ParamMode::Ref)
                .collect(),
            ty: ret.clone(),
        }
    }

    pub(super) fn trait_call(&mut self, depth: usize) -> Option<Expr> {
        let mut targets: Vec<Ty> = self.describes.clone();
        targets.extend(
            self.types
                .iter()
                .filter(|def| def.shape.describe)
                .map(crate::lang::user::UserDef::ty),
        );
        if targets.is_empty() {
            return None;
        }
        let ty = self.pick(&targets).clone();
        let base = unbare_deep(self.typed_only(|inner| inner.expr(&ty, depth - 1)));
        Some(Expr::TraitCall {
            base: Box::new(base),
        })
    }

    /// `value?` where the return type accepts it, through `From` if needed.
    pub(super) fn try_expr(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let ret = self.fn_ret.clone()?;
        let inner = match &ret {
            Ty::Opt(_) => Ty::opt_of(want.clone()),
            Ty::Res(_, err) => {
                let mut sources = vec![(**err).clone()];
                if let Ty::User(shape) = &**err {
                    sources.extend(shape.froms.iter().cloned());
                }
                let source = self.pick(&sources).clone();
                Ty::res_of(want.clone(), source)
            }
            _ => return None,
        };
        let value = self.expr(&inner, depth - 1);
        Some(Expr::Try {
            value: Box::new(value),
            ty: want.clone(),
        })
    }
}
