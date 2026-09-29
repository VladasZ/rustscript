//! Read only walks over the source the compiler still asks about.

use syn::{BinOp, Expr};

use super::first_generic_type;

/// For let chains like `if let A = x && cond && let B = y`.
pub(super) fn flatten_and(cond: &Expr) -> Vec<&Expr> {
    pub(super) fn walk<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
        if let Expr::Binary(b) = e
            && matches!(b.op, BinOp::And(_))
        {
            walk(&b.left, out);
            walk(&b.right, out);
        } else {
            out.push(e);
        }
    }
    let mut out = Vec::new();
    walk(cond, &mut out);
    out
}

/// Looks through `?`, `unwrap`, `expect` and `ok`. A call with its own turbofish doesn't count.
pub(super) fn from_str_root(e: &Expr) -> Option<&syn::ExprCall> {
    let Expr::Call(c) = unwrapped_root(e, false) else {
        return None;
    };
    let Expr::Path(p) = &*c.func else { return None };
    let seg = p.path.segments.last()?;
    if seg.ident != "from_str" || first_generic_type(seg).is_some() {
        return None;
    }
    Some(c)
}

/// A `try_into()` under `?`, `unwrap`, `expect` and `ok`, whose target is the annotation or the
/// argument around it.
pub(super) fn try_into_root(e: &Expr) -> Option<&syn::ExprMethodCall> {
    match unwrapped_root(e, false) {
        Expr::MethodCall(m) if m.method == "try_into" && m.args.is_empty() => Some(m),
        _ => None,
    }
}

/// Rewrites only the error or drops it with `ok`, so the annotation still names the conversion
/// target.
fn keeps_the_payload(method: &syn::Ident) -> bool {
    method == "map_err" || method == "context" || method == "with_context" || method == "ok"
}

/// The payload keeping methods are only followed under a `?`, `unwrap` or `expect`, without one the
/// annotation names a `Result` and not the payload.
fn unwrapped_root(e: &Expr, unwrapped: bool) -> &Expr {
    match e {
        Expr::Try(t) => unwrapped_root(&t.expr, true),
        Expr::Paren(p) => unwrapped_root(&p.expr, unwrapped),
        Expr::Group(g) => unwrapped_root(&g.expr, unwrapped),
        Expr::MethodCall(m) if m.method == "unwrap" || m.method == "expect" => {
            unwrapped_root(&m.receiver, true)
        }
        Expr::MethodCall(m) if unwrapped && keeps_the_payload(&m.method) => {
            unwrapped_root(&m.receiver, unwrapped)
        }
        other => other,
    }
}

pub(super) fn unparen(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(p) => unparen(&p.expr),
        Expr::Group(g) => unparen(&g.expr),
        other => other,
    }
}

/// `<[T]>::len` as `Vec::len`
pub(super) fn qualified_method_ref(p: &syn::ExprPath) -> Vec<String> {
    let owner = match p.qself.as_ref().map(|q| &*q.ty) {
        Some(syn::Type::Path(tp)) => tp
            .path
            .segments
            .last()
            .map_or_else(|| "Vec".to_string(), |s| s.ident.to_string()),
        Some(syn::Type::Slice(_)) => "Vec".to_string(),
        _ => "str".to_string(),
    };
    vec![owner, p.path.segments[0].ident.to_string()]
}
