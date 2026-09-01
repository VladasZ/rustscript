//! Generic parameters of a script signature, bound from the arguments and substituted back.

use std::collections::HashMap;
use std::sync::Arc;

use super::Ty;

pub(super) fn is_self_return(sig: &syn::Signature) -> bool {
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
