//! Path values and path calls, `None`, `Vec::new()`, `String::from(..)`, `fs::read_to_string`,
//! a script function, a constructor of a script type.

use std::collections::HashMap;
use std::sync::Arc;

use syn::Expr;

use super::paths_builtin::numeric_constant;
use super::{Infer, Ty, type_arg};
use crate::interpreter::resolver::Res;

impl Infer<'_, '_> {
    pub(super) fn path_value(&mut self, p: &syn::ExprPath, expected: &Ty) -> Ty {
        if let Some(qself) = &p.qself {
            return self.lower(&qself.ty);
        }
        let segs: Vec<String> = p
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        if let [name] = segs.as_slice() {
            if let Some(ty) = self.lookup(name) {
                return ty;
            }
            if name == "None" {
                let payload = p
                    .path
                    .segments
                    .last()
                    .and_then(|seg| type_arg(seg, 0))
                    .map_or_else(|| expected.payload(), |t| self.lower(t));
                return Ty::option(payload);
            }
        }
        if let Some(ty) = numeric_constant(&segs) {
            return ty;
        }
        let segs = self.self_prefixed(segs);
        match self.ctx.resolver.resolve(self.ctx.module, &segs) {
            Ok(Res::Const(_)) => self
                .ctx
                .const_types
                .get(segs.join("::").as_str())
                .or_else(|| {
                    self.ctx
                        .const_types
                        .get(segs.last().map_or("", String::as_str))
                })
                .map_or(Ty::Unknown, |ty| self.lower(ty)),
            Ok(Res::Struct(canon)) => Ty::Struct(canon),
            Ok(Res::Enum(canon)) => Ty::Enum(canon),
            Ok(Res::TypeMember(canon, rest)) => {
                if self.ctx.resolver.enums.contains_key(&canon) && rest.len() == 1 {
                    return Ty::Enum(canon);
                }
                let key = format!(
                    "{}::{}",
                    crate::interpreter::resolver::bare(&canon),
                    rest.join("::")
                );
                if let Some(ty) = self.ctx.const_types.get(&key) {
                    return self.lower(ty);
                }
                // a method used as a value, `.map(Point::norm)`
                match self
                    .ctx
                    .impl_sigs
                    .get(&(canon.to_string(), rest.join("::")))
                {
                    Some(sig) => self.fn_value(&sig.clone(), Some(&self.user_type(&canon))),
                    None => Ty::Unknown,
                }
            }
            Ok(Res::Fn(_)) => match self
                .ctx
                .fn_signatures
                .get(segs.last().map_or("", String::as_str))
            {
                Some(sig) => self.fn_value(&sig.clone(), None),
                None => Ty::Unknown,
            },
            _ => Ty::Unknown,
        }
    }

    /// A function as a closure value.
    fn fn_value(&mut self, sig: &syn::Signature, self_ty: Option<&Ty>) -> Ty {
        let saved = self.swap_generics(sig);
        let params = sig
            .inputs
            .iter()
            .map(|input| match input {
                syn::FnArg::Receiver(_) => self_ty.cloned().unwrap_or(Ty::Unknown),
                syn::FnArg::Typed(t) => self.lower(&t.ty),
            })
            .collect();
        let ret = match &sig.output {
            syn::ReturnType::Type(_, ty) => self.lower(ty),
            syn::ReturnType::Default => Ty::Unit,
        };
        self.generics = saved;
        let ret = match (ret, self_ty) {
            (Ty::Unknown, Some(s)) if is_self_return(sig) => s.clone(),
            (ret, _) => ret,
        };
        Ty::Closure(params, Box::new(ret))
    }

    fn self_prefixed(&self, mut segs: Vec<String>) -> Vec<String> {
        if segs.first().is_some_and(|s| s == "Self")
            && let Some(ty) = self.ctx.impl_type
        {
            segs[0] = ty.to_string();
        }
        segs
    }

    /// `Some`, `Ok`, `Err`, `drop` and a local closure called by name.
    fn prelude_call(
        &mut self,
        name: &str,
        seg: &syn::PathSegment,
        args: &[&Expr],
        expected: &Ty,
    ) -> Option<Ty> {
        // `Ok::<T, E>(..)` states both sides, the expectation fills what it leaves out
        let stated = |i: usize| type_arg(seg, i).map(|t| self.lower(t));
        match name {
            "Some" => {
                let want = stated(0).unwrap_or_else(|| expected.payload());
                let payload = self.arg(args, 0, &want);
                return Some(Ty::option(payload));
            }
            "Ok" | "Err" => {
                let (ok, err) = match expected {
                    Ty::Result(ok, err) => ((**ok).clone(), (**err).clone()),
                    _ => (Ty::Unknown, Ty::Unknown),
                };
                let ok = stated(0).unwrap_or(ok);
                let err = stated(1).unwrap_or(err);
                if name == "Ok" {
                    let ok = self.arg(args, 0, &ok);
                    return Some(Ty::result(ok, err));
                }
                let err = self.arg(args, 0, &err);
                return Some(Ty::result(ok, err));
            }
            "drop" => {
                self.arg(args, 0, &Ty::Unknown);
                return Some(Ty::Unit);
            }
            _ => {}
        }
        if let Some(Ty::Closure(params, ret)) = self.lookup(name) {
            for (i, arg) in args.iter().enumerate() {
                let want = params.get(i).cloned().unwrap_or(Ty::Unknown);
                self.expr(arg, &want);
            }
            return Some(*ret);
        }
        None
    }

    pub(super) fn call(&mut self, c: &syn::ExprCall, expected: &Ty) -> Ty {
        let Expr::Path(path) = &*c.func else {
            // a closure value
            let callee = self.expr(&c.func, &Ty::Unknown);
            let (params, ret) = match callee {
                Ty::Closure(params, ret) => (params, *ret),
                _ => (Vec::new(), Ty::Unknown),
            };
            for (i, arg) in c.args.iter().enumerate() {
                let want = params.get(i).cloned().unwrap_or(Ty::Unknown);
                self.expr(arg, &want);
            }
            return ret;
        };
        if let Some(qself) = &path.qself {
            let owner = self.lower(&qself.ty);
            let name = path
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default();
            return self.assoc_call(&owner, &name, &path.path, c, expected);
        }
        let segs: Vec<String> = path
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        let args: Vec<&Expr> = c.args.iter().collect();
        if let [name] = segs.as_slice()
            && let Some(ty) = self.prelude_call(name, &path.path.segments[0], &args, expected)
        {
            return ty;
        }
        let segs = self.self_prefixed(segs);
        match self.ctx.resolver.resolve(self.ctx.module, &segs) {
            Ok(Res::Fn(_)) => {
                let name = segs.last().cloned().unwrap_or_default();
                match self.ctx.fn_signatures.get(&name).cloned() {
                    Some(sig) => self.sig_call(&sig, None, &args, expected),
                    None => self.walk_args(&args),
                }
            }
            Ok(Res::Struct(canon)) => {
                let fields = self.variant_fields(&path.path, &Ty::Struct(canon.clone()));
                for (i, arg) in args.iter().enumerate() {
                    let want = fields.get(i).map_or(Ty::Unknown, |(_, t)| t.clone());
                    self.expr(arg, &want);
                }
                Ty::Struct(canon)
            }
            Ok(Res::TypeMember(canon, rest)) => {
                let owner = self.user_type(&canon);
                if let Ty::Enum(_) = owner
                    && let [variant] = rest.as_slice()
                    && self
                        .ctx
                        .resolver
                        .enums
                        .get(&canon)
                        .is_some_and(|e| e.variants.iter().any(|v| v.ident == variant))
                {
                    let fields = self.variant_payload(&canon, variant);
                    for (i, arg) in args.iter().enumerate() {
                        let want = fields.get(i).map_or(Ty::Unknown, |(_, t)| t.clone());
                        self.expr(arg, &want);
                    }
                    return owner;
                }
                self.assoc_call(&owner, &rest.join("::"), &path.path, c, expected)
            }
            Ok(Res::Alias(m, target)) => {
                let owner = self.lower_in(&target, m);
                let name = segs.last().cloned().unwrap_or_default();
                self.assoc_call(&owner, &name, &path.path, c, expected)
            }
            _ => self.external_call(&segs, &path.path, &args, expected),
        }
    }

    /// `Type::name(..)` on a script type or a bridge type.
    fn assoc_call(
        &mut self,
        owner: &Ty,
        name: &str,
        path: &syn::Path,
        c: &syn::ExprCall,
        expected: &Ty,
    ) -> Ty {
        let args: Vec<&Expr> = c.args.iter().collect();
        let canon = match owner {
            Ty::Struct(c) | Ty::Enum(c) => Some(c.clone()),
            _ => None,
        };
        if let Some(canon) = canon
            && let Some(sig) = self
                .ctx
                .impl_sigs
                .get(&(canon.to_string(), name.to_string()))
                .cloned()
        {
            return self.sig_call(&sig, Some(owner), &args, expected);
        }
        if name == "default" && args.is_empty() {
            return owner.clone();
        }
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        self.external_call(&segs, path, &args, expected)
    }

    pub(super) fn walk_args(&mut self, args: &[&Expr]) -> Ty {
        for arg in args {
            self.expr(arg, &Ty::Unknown);
        }
        Ty::Unknown
    }

    pub(super) fn arg(&mut self, args: &[&Expr], i: usize, want: &Ty) -> Ty {
        match args.get(i) {
            Some(arg) => self.expr(arg, want),
            None => Ty::Unknown,
        }
    }

    /// The generics of a signature shadow the enclosing ones while it is lowered.
    pub(super) fn swap_generics(&mut self, sig: &syn::Signature) -> Vec<Arc<str>> {
        let own: Vec<Arc<str>> = sig
            .generics
            .type_params()
            .map(|p| Arc::from(p.ident.to_string().as_str()))
            .collect();
        std::mem::replace(&mut self.generics, own)
    }

    /// A call through a signature. Generic parameters take the type of the argument passed for
    /// them, so `fn pick<T>(a: T) -> T` returns what it was given.
    pub(super) fn sig_call(
        &mut self,
        sig: &syn::Signature,
        self_ty: Option<&Ty>,
        args: &[&Expr],
        expected: &Ty,
    ) -> Ty {
        let saved = self.swap_generics(sig);
        let params: Vec<Ty> = sig
            .inputs
            .iter()
            .filter_map(|input| match input {
                syn::FnArg::Receiver(_) => None,
                syn::FnArg::Typed(t) => Some(self.lower(&t.ty)),
            })
            .collect();
        let ret = match &sig.output {
            syn::ReturnType::Type(_, ty) => self.lower(ty),
            syn::ReturnType::Default => Ty::Unit,
        };
        self.generics = saved;
        let mut bound: HashMap<Arc<str>, Ty> = HashMap::new();
        // The expectation only hints a literal argument and fills a parameter no argument bound.
        // The arguments win, `E::from(pick(5usize, 6, false))` expects the payload of some `From`
        // impl and that must not decide what `pick` returns.
        let mut hint: HashMap<Arc<str>, Ty> = HashMap::new();
        bind_generic(&ret, expected, &mut hint);
        // the receiver of a method call sits before the typed params
        let skip = usize::from(sig.receiver().is_some());
        for (i, arg) in args.iter().enumerate() {
            let param = params
                .get(i.saturating_sub(skip))
                .cloned()
                .unwrap_or(Ty::Unknown);
            let param = if self_ty.is_some() && i < skip {
                Ty::Unknown
            } else {
                param
            };
            let want = subst(&subst(&param, &bound), &hint);
            let got = self.expr(arg, &erase(&want));
            bind_generic(&param, &got, &mut bound);
        }
        let ret = subst(&subst(&ret, &bound), &hint);
        match (&ret, self_ty) {
            (Ty::Unknown, Some(s)) if is_self_return(sig) => s.clone(),
            _ => erase(&ret),
        }
    }
}

fn is_self_return(sig: &syn::Signature) -> bool {
    matches!(&sig.output, syn::ReturnType::Type(_, ty)
        if matches!(&**ty, syn::Type::Path(p) if p.path.is_ident("Self")))
}

/// Records what a generic parameter stands for, from a parameter type against an argument type.
pub(super) fn bind_generic(param: &Ty, arg: &Ty, bound: &mut HashMap<Arc<str>, Ty>) {
    match (param, arg) {
        (Ty::Generic(name), got) if !got.is_unknown() && !matches!(got, Ty::Generic(_)) => {
            bound.entry(name.clone()).or_insert_with(|| got.clone());
        }
        (Ty::Vec(p), Ty::Vec(a))
        | (Ty::Set(p), Ty::Set(a))
        | (Ty::Option(p), Ty::Option(a))
        | (Ty::Iter(p), Ty::Iter(a) | Ty::Vec(a))
        | (Ty::Range(p), Ty::Range(a)) => bind_generic(p, a, bound),
        (Ty::Map(pk, pv), Ty::Map(ak, av)) | (Ty::Result(pk, pv), Ty::Result(ak, av)) => {
            bind_generic(pk, ak, bound);
            bind_generic(pv, av, bound);
        }
        (Ty::Tuple(ps), Ty::Tuple(xs)) if ps.len() == xs.len() => {
            for (p, x) in ps.iter().zip(xs) {
                bind_generic(p, x, bound);
            }
        }
        (Ty::Closure(pp, pr), Ty::Closure(ap, ar)) if pp.len() == ap.len() => {
            for (p, x) in pp.iter().zip(ap) {
                bind_generic(p, x, bound);
            }
            bind_generic(pr, ar, bound);
        }
        _ => {}
    }
}

pub(super) fn subst(ty: &Ty, bound: &HashMap<Arc<str>, Ty>) -> Ty {
    match ty {
        Ty::Generic(name) => bound.get(name).cloned().unwrap_or_else(|| ty.clone()),
        Ty::Vec(t) => Ty::vec(subst(t, bound)),
        Ty::Set(t) => Ty::Set(Box::new(subst(t, bound))),
        Ty::Option(t) => Ty::option(subst(t, bound)),
        Ty::Iter(t) => Ty::iter(subst(t, bound)),
        Ty::Range(t) => Ty::Range(Box::new(subst(t, bound))),
        Ty::Entry(t) => Ty::Entry(Box::new(subst(t, bound))),
        Ty::Map(k, v) => Ty::Map(Box::new(subst(k, bound)), Box::new(subst(v, bound))),
        Ty::Result(k, v) => Ty::result(subst(k, bound), subst(v, bound)),
        Ty::Tuple(items) => Ty::Tuple(items.iter().map(|t| subst(t, bound)).collect()),
        Ty::Closure(params, ret) => Ty::Closure(
            params.iter().map(|t| subst(t, bound)).collect(),
            Box::new(subst(ret, bound)),
        ),
        Ty::Named(name, args) => {
            Ty::Named(name.clone(), args.iter().map(|t| subst(t, bound)).collect())
        }
        other => other.clone(),
    }
}

/// An unbound generic is no information.
pub(super) fn erase(ty: &Ty) -> Ty {
    match ty {
        Ty::Generic(_) => Ty::Unknown,
        Ty::Vec(t) => Ty::vec(erase(t)),
        Ty::Set(t) => Ty::Set(Box::new(erase(t))),
        Ty::Option(t) => Ty::option(erase(t)),
        Ty::Iter(t) => Ty::iter(erase(t)),
        Ty::Range(t) => Ty::Range(Box::new(erase(t))),
        Ty::Entry(t) => Ty::Entry(Box::new(erase(t))),
        Ty::Map(k, v) => Ty::Map(Box::new(erase(k)), Box::new(erase(v))),
        Ty::Result(k, v) => Ty::result(erase(k), erase(v)),
        Ty::Tuple(items) => Ty::Tuple(items.iter().map(erase).collect()),
        Ty::Closure(params, ret) => {
            Ty::Closure(params.iter().map(erase).collect(), Box::new(erase(ret)))
        }
        Ty::Named(name, args) => Ty::Named(name.clone(), args.iter().map(erase).collect()),
        other => other.clone(),
    }
}
