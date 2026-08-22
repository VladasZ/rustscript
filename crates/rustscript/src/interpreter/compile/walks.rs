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

/// Looks through `?`, `unwrap` and `expect`. A call with its own turbofish doesn't count.
pub(super) fn from_str_root(e: &Expr) -> Option<&syn::ExprCall> {
    from_str_chain(e, false)
}

/// Rewrites only the error, so the annotation still names the parse target.
fn maps_only_the_error(method: &syn::Ident) -> bool {
    method == "map_err" || method == "context" || method == "with_context"
}

/// The error mapping methods are only followed under a `?`, `unwrap` or `expect`, without one the
/// annotation names a `Result` and not the payload.
fn from_str_chain(e: &Expr, unwrapped: bool) -> Option<&syn::ExprCall> {
    match e {
        Expr::Call(c) => {
            let Expr::Path(p) = &*c.func else { return None };
            let seg = p.path.segments.last()?;
            if seg.ident != "from_str" || first_generic_type(seg).is_some() {
                return None;
            }
            Some(c)
        }
        Expr::Try(t) => from_str_chain(&t.expr, true),
        Expr::Paren(p) => from_str_chain(&p.expr, unwrapped),
        Expr::Group(g) => from_str_chain(&g.expr, unwrapped),
        Expr::MethodCall(m) if m.method == "unwrap" || m.method == "expect" => {
            from_str_chain(&m.receiver, true)
        }
        Expr::MethodCall(m) if unwrapped && maps_only_the_error(&m.method) => {
            from_str_chain(&m.receiver, unwrapped)
        }
        _ => None,
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
