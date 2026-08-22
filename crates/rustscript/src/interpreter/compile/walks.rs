//! Read only walks over the source, the tails, returns and literal shapes the compiler asks about.

use syn::{BinOp, Block, Expr, Lit, Stmt, UnOp};

use crate::interpreter::bytecode::BinKind;

use super::written::block_tail;

use super::{CollectTarget, ScalarTy, bin_kind, first_generic_type, is_assign_op};

/// Operators whose operands share the annotated integer type. Comparisons give bool.
pub(super) fn propagates_annotation(op: BinKind) -> bool {
    matches!(
        op,
        BinKind::Add
            | BinKind::Sub
            | BinKind::Mul
            | BinKind::Div
            | BinKind::Rem
            | BinKind::BitAnd
            | BinKind::BitOr
            | BinKind::BitXor
            | BinKind::Shl
            | BinKind::Shr
    )
}

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
pub(super) fn maps_only_the_error(method: &syn::Ident) -> bool {
    method == "map_err" || method == "context" || method == "with_context"
}

/// The error mapping methods are only followed under a `?`, `unwrap` or `expect`, without one the
/// annotation names a `Result` and not the payload.
pub(super) fn from_str_chain(e: &Expr, unwrapped: bool) -> Option<&syn::ExprCall> {
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

/// A call with its own turbofish doesn't count.
pub(super) fn collect_root(e: &Expr) -> Option<&syn::ExprMethodCall> {
    match e {
        Expr::MethodCall(m) if m.method == "collect" && m.turbofish.is_none() => Some(m),
        Expr::Paren(p) => collect_root(&p.expr),
        Expr::Group(g) => collect_root(&g.expr),
        _ => None,
    }
}

/// The collect target a return type names.
pub(super) fn collect_return_target(output: &syn::ReturnType) -> Option<CollectTarget> {
    match output {
        syn::ReturnType::Type(_, ty) => CollectTarget::of_type(ty),
        syn::ReturnType::Default => None,
    }
}

/// The payload a signature hands back, inside a `Result`, which a tail `from_str` must parse into.
pub(super) fn returned_json_type(output: &syn::ReturnType) -> Option<&syn::Type> {
    let syn::ReturnType::Type(_, ty) = output else {
        return None;
    };
    Some(result_ok_type(ty).unwrap_or(ty))
}

pub(super) fn result_ok_type(ty: &syn::Type) -> Option<&syn::Type> {
    let syn::Type::Path(p) = ty else { return None };
    let seg = p.path.segments.last()?;
    if seg.ident != "Result" {
        return None;
    }
    first_generic_type(seg)
}

/// Parens, a block's tail, both branches of an `if` and every arm of a `match`.
pub(super) fn tail_exprs<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
    match e {
        Expr::Paren(p) => tail_exprs(&p.expr, out),
        Expr::Group(g) => tail_exprs(&g.expr, out),
        Expr::Block(b) => tail_block_exprs(&b.block, out),
        Expr::If(i) => {
            tail_block_exprs(&i.then_branch, out);
            if let Some((_, alt)) = &i.else_branch {
                tail_exprs(alt, out);
            }
        }
        Expr::Match(m) => {
            for arm in &m.arms {
                tail_exprs(&arm.body, out);
            }
        }
        other => out.push(other),
    }
}

/// Only unsuffixed integer literals, so `rustc` falls back to `i32`.
pub(super) fn unconstrained_int(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(l) => matches!(&l.lit, Lit::Int(int) if int.suffix().is_empty()),
        Expr::Paren(p) => unconstrained_int(&p.expr),
        Expr::Group(g) => unconstrained_int(&g.expr),
        Expr::Unary(u) => matches!(u.op, UnOp::Neg(_)) && unconstrained_int(&u.expr),
        Expr::Binary(b) => match bin_kind(&b.op) {
            Some(BinKind::Shl | BinKind::Shr) => unconstrained_int(&b.left),
            Some(op) if propagates_annotation(op) && !is_assign_op(&b.op) => {
                unconstrained_int(&b.left) && unconstrained_int(&b.right)
            }
            _ => false,
        },
        Expr::If(_) | Expr::Block(_) | Expr::Match(_) => {
            let mut tails = Vec::new();
            tail_exprs(expr, &mut tails);
            let complete = match expr {
                Expr::If(sel) => sel.else_branch.is_some(),
                _ => true,
            };
            complete && !tails.is_empty() && tails.into_iter().all(unconstrained_int)
        }
        _ => false,
    }
}

/// Only a computation can overflow, and a plain `let x = 0` must stay open so a later `x +=
/// v.len()` can make it a `usize`.
pub(super) fn bare_int_arithmetic(expr: &Expr) -> bool {
    match expr {
        Expr::Unary(un) => matches!(un.op, UnOp::Neg(_)),
        Expr::Binary(_) => true,
        Expr::Paren(inner) => bare_int_arithmetic(&inner.expr),
        Expr::Group(inner) => bare_int_arithmetic(&inner.expr),
        Expr::If(sel) => {
            let then = block_tail(&sel.then_branch).is_some_and(bare_int_arithmetic);
            let other = sel
                .else_branch
                .as_ref()
                .is_some_and(|(_, e)| bare_int_arithmetic(e));
            then || other
        }
        Expr::Block(block) => block_tail(&block.block).is_some_and(bare_int_arithmetic),
        Expr::Match(m) => m.arms.iter().any(|arm| bare_int_arithmetic(&arm.body)),
        _ => false,
    }
}

/// Every numeric leaf is an unsuffixed literal, so arithmetic overflows at `i32`.
pub(super) fn bare_int_rooted(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(lit) => match &lit.lit {
            syn::Lit::Int(int) => int.suffix().is_empty(),
            _ => false,
        },
        Expr::Paren(inner) => bare_int_rooted(&inner.expr),
        Expr::Group(inner) => bare_int_rooted(&inner.expr),
        Expr::Unary(un) => matches!(un.op, UnOp::Neg(_)) && bare_int_rooted(&un.expr),
        Expr::Binary(bin) => bare_int_rooted(&bin.left) && bare_int_rooted(&bin.right),
        Expr::If(sel) => {
            let Some(then) = block_tail(&sel.then_branch) else {
                return false;
            };
            let Some((_, other)) = sel.else_branch.as_ref() else {
                return false;
            };
            bare_int_rooted(then) && bare_int_rooted(other)
        }
        Expr::Block(block) => block_tail(&block.block).is_some_and(bare_int_rooted),
        Expr::Match(m) => !m.arms.is_empty() && m.arms.iter().all(|arm| bare_int_rooted(&arm.body)),
        _ => false,
    }
}

pub(super) fn takes_numeric_hint(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(l) => matches!(&l.lit, Lit::Int(_) | Lit::Float(_)),
        Expr::Paren(p) => takes_numeric_hint(&p.expr),
        Expr::Group(g) => takes_numeric_hint(&g.expr),
        Expr::Unary(u) => matches!(u.op, UnOp::Neg(_)),
        Expr::Binary(b) => {
            !is_assign_op(&b.op) && bin_kind(&b.op).is_some_and(propagates_annotation)
        }
        Expr::If(sel) => sel.else_branch.is_some(),
        Expr::Block(_) | Expr::Match(_) => true,
        _ => false,
    }
}

pub(super) fn unparen(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(p) => unparen(&p.expr),
        Expr::Group(g) => unparen(&g.expr),
        other => other,
    }
}

pub(super) fn tail_block_exprs<'a>(block: &'a Block, out: &mut Vec<&'a Expr>) {
    if let Some(Stmt::Expr(e, None)) = block.stmts.last() {
        tail_exprs(e, out);
    }
}

/// The tail and every `return`. A closure body is skipped, its `return` leaves the closure.
pub(super) fn returned_exprs(block: &Block) -> Vec<&Expr> {
    let mut found = Vec::new();
    tail_block_exprs(block, &mut found);
    walk_returns(block, &mut found);
    found
}

pub(super) fn returned_collects(block: &Block) -> Vec<*const syn::ExprMethodCall> {
    returned_exprs(block)
        .into_iter()
        .filter_map(|e| match e {
            Expr::MethodCall(m) if m.method == "collect" && m.turbofish.is_none() => {
                Some(std::ptr::from_ref(m))
            }
            _ => None,
        })
        .collect()
}

/// Walked as already unwrapped, because the payload is read from inside the `Result` of the signature.
pub(super) fn returned_from_strs(block: &Block) -> Vec<*const syn::ExprCall> {
    returned_exprs(block)
        .into_iter()
        .filter_map(|e| from_str_chain(e, true).map(std::ptr::from_ref))
        .collect()
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

pub(super) fn walk_returns<'a>(block: &'a Block, out: &mut Vec<&'a Expr>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Expr(e, _) => walk_returns_expr(e, out),
            // the `else` block of a let else commonly returns
            Stmt::Local(local) => {
                if let Some(init) = &local.init
                    && let Some(diverge) = &init.diverge
                {
                    walk_returns_expr(&diverge.1, out);
                }
            }
            _ => {}
        }
    }
}

pub(super) fn walk_returns_expr<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
    match e {
        Expr::Return(r) => {
            if let Some(value) = &r.expr {
                tail_exprs(value, out);
            }
        }
        Expr::Block(b) => walk_returns(&b.block, out),
        Expr::Unsafe(u) => walk_returns(&u.block, out),
        Expr::If(i) => {
            walk_returns(&i.then_branch, out);
            if let Some((_, alt)) = &i.else_branch {
                walk_returns_expr(alt, out);
            }
        }
        Expr::Match(m) => {
            for arm in &m.arms {
                walk_returns_expr(&arm.body, out);
            }
        }
        Expr::ForLoop(f) => walk_returns(&f.body, out),
        Expr::While(w) => walk_returns(&w.body, out),
        Expr::Loop(l) => walk_returns(&l.body, out),
        _ => {}
    }
}

/// For building a `Default` further down the chain.
pub(super) fn annotation_scalar(ty: &syn::Type) -> Option<ScalarTy> {
    // a `Result<T, E>` goes through the `Opt` shape, only the payload side ever builds a default
    if let syn::Type::Path(path) = ty
        && let Some(segment) = path.path.segments.last()
        && segment.ident == "Result"
        && let syn::PathArguments::AngleBracketed(args) = &segment.arguments
    {
        let inner = args.args.iter().find_map(|arg| match arg {
            syn::GenericArgument::Type(inner) => ScalarTy::lower(inner),
            _ => None,
        })?;
        return Some(ScalarTy::Opt(Box::new(inner)));
    }
    ScalarTy::lower(ty)
}

pub(super) fn sequence_element(p: &syn::TypePath) -> Option<&syn::Type> {
    generic_named(p, "Vec").or_else(|| generic_named(p, "VecDeque"))
}

pub(super) fn generic_named<'t>(p: &'t syn::TypePath, name: &str) -> Option<&'t syn::Type> {
    let seg = p.path.segments.last()?;
    if seg.ident != name {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        syn::GenericArgument::Type(ty) => Some(ty),
        _ => None,
    })
}
