//! Small helpers shared by the compile modules.

use anyhow::Result;
use syn::punctuated::Punctuated;
use syn::{BinOp, Expr, Lit, Pat, UnOp};

use crate::interpreter::bytecode::BinKind;
use crate::interpreter::numeric::IntWidth;

pub(super) fn is_assign_op(op: &BinOp) -> bool {
    use BinOp::{
        AddAssign, BitAndAssign, BitOrAssign, BitXorAssign, DivAssign, MulAssign, RemAssign,
        ShlAssign, ShrAssign, SubAssign,
    };
    matches!(
        op,
        AddAssign(_)
            | SubAssign(_)
            | MulAssign(_)
            | DivAssign(_)
            | RemAssign(_)
            | BitAndAssign(_)
            | BitOrAssign(_)
            | BitXorAssign(_)
            | ShlAssign(_)
            | ShrAssign(_)
    )
}

pub(super) fn bin_kind(op: &BinOp) -> Option<BinKind> {
    use BinOp::{
        Add, AddAssign, BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, Div,
        DivAssign, Eq, Ge, Gt, Le, Lt, Mul, MulAssign, Ne, Rem, RemAssign, Shl, ShlAssign, Shr,
        ShrAssign, Sub, SubAssign,
    };
    Some(match op {
        Add(_) | AddAssign(_) => BinKind::Add,
        Sub(_) | SubAssign(_) => BinKind::Sub,
        Mul(_) | MulAssign(_) => BinKind::Mul,
        Div(_) | DivAssign(_) => BinKind::Div,
        Rem(_) | RemAssign(_) => BinKind::Rem,
        Eq(_) => BinKind::Eq,
        Ne(_) => BinKind::Ne,
        Lt(_) => BinKind::Lt,
        Le(_) => BinKind::Le,
        Gt(_) => BinKind::Gt,
        Ge(_) => BinKind::Ge,
        BitAnd(_) | BitAndAssign(_) => BinKind::BitAnd,
        BitOr(_) | BitOrAssign(_) => BinKind::BitOr,
        BitXor(_) | BitXorAssign(_) => BinKind::BitXor,
        Shl(_) | ShlAssign(_) => BinKind::Shl,
        Shr(_) | ShrAssign(_) => BinKind::Shr,
        _ => return None,
    })
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(in crate::interpreter) enum FloatTy {
    F32,
    F64,
}

/// A non literal init retags through a runtime cast, a no-op on an already typed value.
#[derive(Clone, Copy)]
pub(in crate::interpreter) enum NumericTy {
    Int(IntWidth),
    Float(FloatTy),
}

pub(super) fn numeric_annotation(ty: &syn::Type) -> Option<NumericTy> {
    let syn::Type::Path(p) = ty else {
        return None;
    };
    let seg = p.path.segments.last()?;
    if !matches!(seg.arguments, syn::PathArguments::None) {
        return None;
    }
    let name = seg.ident.to_string();
    match name.as_str() {
        "f32" => Some(NumericTy::Float(FloatTy::F32)),
        "f64" => Some(NumericTy::Float(FloatTy::F64)),
        _ => IntWidth::parse(&name).map(NumericTy::Int),
    }
}

/// Including a negated one, seen through parens.
pub(super) fn int_literal(e: &Expr) -> Option<i64> {
    match e {
        Expr::Lit(l) => match &l.lit {
            Lit::Int(i) => i.base10_parse::<i64>().ok(),
            Lit::Byte(b) => Some(i64::from(b.value())),
            _ => None,
        },
        Expr::Unary(u) if matches!(u.op, UnOp::Neg(_)) => match &*u.expr {
            Expr::Lit(l) => match &l.lit {
                Lit::Int(i) => i.base10_parse::<i64>().ok().map(|v| -v),
                _ => None,
            },
            _ => None,
        },
        Expr::Paren(p) => int_literal(&p.expr),
        Expr::Group(g) => int_literal(&g.expr),
        _ => None,
    }
}

pub fn first_generic_type(seg: &syn::PathSegment) -> Option<&syn::Type> {
    if let syn::PathArguments::AngleBracketed(ab) = &seg.arguments {
        for a in &ab.args {
            if let syn::GenericArgument::Type(t) = a {
                return Some(t);
            }
        }
    }
    None
}

pub(super) fn collect_pattern_names(pat: &Pat, out: &mut Vec<String>) {
    match pat {
        Pat::Ident(id) if super::pattern::is_unit_variant_ident(id) => {}
        Pat::Ident(id) => {
            out.push(id.ident.to_string());
            if let Some(sub) = &id.subpat {
                collect_pattern_names(&sub.1, out);
            }
        }
        Pat::Tuple(t) => t.elems.iter().for_each(|p| collect_pattern_names(p, out)),
        Pat::TupleStruct(ts) => ts.elems.iter().for_each(|p| collect_pattern_names(p, out)),
        Pat::Slice(s) => s.elems.iter().for_each(|p| collect_pattern_names(p, out)),
        Pat::Struct(s) => s
            .fields
            .iter()
            .for_each(|f| collect_pattern_names(&f.pat, out)),
        Pat::Reference(r) => collect_pattern_names(&r.pat, out),
        Pat::Paren(p) => collect_pattern_names(&p.pat, out),
        Pat::Type(t) => collect_pattern_names(&t.pat, out),
        Pat::Or(o) => {
            // every alternative binds the same names
            if let Some(first) = o.cases.first() {
                collect_pattern_names(first, out);
            }
        }
        _ => {}
    }
}

pub(super) fn is_name(arg: &str) -> bool {
    !arg.is_empty()
        && arg.parse::<usize>().is_err()
        && arg.chars().all(|c| c.is_alphanumeric() || c == '_')
        && arg
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
}

pub(super) fn inline_holes(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            if chars.peek() == Some(&'{') {
                chars.next();
                continue;
            }
            let mut inner = String::new();
            for ic in chars.by_ref() {
                if ic == '}' {
                    break;
                }
                inner.push(ic);
            }
            // `{:w$}` names a variable after the colon, that is a hole too
            if let Some((_, spec)) = inner.split_once(':') {
                let mut token = String::new();
                for c in spec.chars() {
                    if c.is_alphanumeric() || c == '_' {
                        token.push(c);
                        continue;
                    }
                    if c == '$' && is_name(&token) {
                        out.push(token.clone());
                    }
                    token.clear();
                }
            }
            let arg = inner.split(':').next().unwrap_or("").trim();
            if is_name(arg) {
                out.push(arg.to_string());
            }
        } else if c == '}' && chars.peek() == Some(&'}') {
            chars.next();
        }
    }
    out
}

pub(super) fn macro_yields_value(mac: &syn::Macro) -> bool {
    let name = mac
        .path
        .segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_default();
    matches!(name.as_str(), "format" | "vec" | "matches" | "dbg")
}

pub(super) fn parse_exprs(mac: &syn::Macro) -> Result<Vec<Expr>> {
    Ok(mac
        .parse_body_with(Punctuated::<Expr, syn::Token![,]>::parse_terminated)?
        .into_iter()
        .collect())
}

pub(super) fn parse_vec_repeat(input: syn::parse::ParseStream) -> syn::Result<(Expr, Expr)> {
    let value: Expr = input.parse()?;
    input.parse::<syn::Token![;]>()?;
    let count: Expr = input.parse()?;
    Ok((value, count))
}

pub(super) fn parse_matches(mac: &syn::Macro) -> Result<(Expr, syn::Pat, Option<Expr>)> {
    pub(super) fn inner(
        input: syn::parse::ParseStream,
    ) -> syn::Result<(Expr, syn::Pat, Option<Expr>)> {
        let expr: Expr = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let pat = syn::Pat::parse_multi_with_leading_vert(input)?;
        let guard = if input.peek(syn::Token![if]) {
            input.parse::<syn::Token![if]>()?;
            Some(input.parse()?)
        } else {
            None
        };
        Ok((expr, pat, guard))
    }
    Ok(mac.parse_body_with(inner)?)
}

/// `&serde_json::Value` is `Value` and `&[String]` is `Vec`. The coverage check only asks what
/// the receiver is.
pub(super) fn type_head(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Reference(r) => type_head(&r.elem),
        syn::Type::Paren(p) => type_head(&p.elem),
        syn::Type::Group(g) => type_head(&g.elem),
        syn::Type::Slice(_) | syn::Type::Array(_) => Some("Vec".to_string()),
        syn::Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}

pub(super) fn expr_kind(expr: &Expr) -> &'static str {
    match expr {
        Expr::Infer(_) => "_ placeholder",
        Expr::Let(_) => "let expression",
        Expr::TryBlock(_) => "try block",
        Expr::Yield(_) => "yield",
        Expr::Const(_) => "const block",
        Expr::Verbatim(_) => "unparsed tokens",
        _ => "this expression",
    }
}

/// Whether the pattern binds a name by value, `Some(x)`, which moves or copies out of the
/// scrutinee. `ref` bindings and wildcards do not.
pub(super) fn pattern_owns(pat: &Pat) -> bool {
    match pat {
        Pat::Ident(id) if super::pattern::is_unit_variant_ident(id) => false,
        Pat::Ident(id) => {
            id.by_ref.is_none() || id.subpat.as_ref().is_some_and(|sub| pattern_owns(&sub.1))
        }
        Pat::Tuple(t) => t.elems.iter().any(pattern_owns),
        Pat::TupleStruct(ts) => ts.elems.iter().any(pattern_owns),
        Pat::Slice(s) => s.elems.iter().any(pattern_owns),
        Pat::Struct(s) => s.fields.iter().any(|f| pattern_owns(&f.pat)),
        Pat::Paren(p) => pattern_owns(&p.pat),
        Pat::Type(t) => pattern_owns(&t.pat),
        Pat::Or(o) => o.cases.iter().any(pattern_owns),
        // a `&x` pattern binds through a reference, the scrutinee is not moved
        _ => false,
    }
}

/// Whether the pattern has a `ref` binding, which must see the scrutinee's own storage.
pub(super) fn pattern_borrows(pat: &Pat) -> bool {
    match pat {
        Pat::Ident(id) => {
            id.by_ref.is_some()
                || id
                    .subpat
                    .as_ref()
                    .is_some_and(|sub| pattern_borrows(&sub.1))
        }
        Pat::Tuple(t) => t.elems.iter().any(pattern_borrows),
        Pat::TupleStruct(ts) => ts.elems.iter().any(pattern_borrows),
        Pat::Slice(s) => s.elems.iter().any(pattern_borrows),
        Pat::Struct(s) => s.fields.iter().any(|f| pattern_borrows(&f.pat)),
        Pat::Paren(p) => pattern_borrows(&p.pat),
        Pat::Type(t) => pattern_borrows(&t.pat),
        Pat::Reference(r) => pattern_borrows(&r.pat),
        Pat::Or(o) => o.cases.iter().any(pattern_borrows),
        _ => false,
    }
}

/// Whether a binding holds an iterator chain that owns its items, see `chain_owns_items`.
pub(super) type BindingOwns<'a> = &'a dyn Fn(&str) -> bool;

/// `for x in EXPR` consumes `EXPR` unless it is a borrow or an iterator method on a borrowed
/// receiver. Only a consumed vec hands its items to the loop.
pub(super) fn iterable_is_owned(expr: &Expr, binding_owns: BindingOwns) -> bool {
    match expr {
        Expr::Paren(p) => iterable_is_owned(&p.expr, binding_owns),
        Expr::Group(g) => iterable_is_owned(&g.expr, binding_owns),
        Expr::Reference(_) | Expr::Range(_) => false,
        // a fresh collection or an owning chain, `v.clone()` or `v.into_iter().rev()`, is the
        // loop's own. Shared with a statement end temporary drop it would drop twice.
        Expr::MethodCall(_) => {
            init_is_owned(expr, binding_owns) || chain_owns_items(expr, binding_owns)
        }
        _ => true,
    }
}

/// Whether a `let mut` init already owns unique storage. A local or a place read is handled by
/// `Own`, a constructor or a call is fresh, and a method in the list hands back a fresh value.
/// Anything else may share storage with a live value and the binding copies first.
pub(super) fn init_is_unique(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(p) => init_is_unique(&p.expr),
        Expr::Group(g) => init_is_unique(&g.expr),
        Expr::Try(t) => init_is_unique(&t.expr),
        Expr::Await(a) => init_is_unique(&a.base),
        Expr::MethodCall(m) => matches!(
            m.method.to_string().as_str(),
            "clone"
                | "cloned"
                | "copied"
                | "to_vec"
                | "to_owned"
                | "to_string"
                | "collect"
                | "pop"
                | "remove"
                | "take"
                | "replace"
                | "swap_remove"
                | "split_off"
                | "drain"
                | "into_iter"
                | "iter"
                | "iter_mut"
                | "chars"
                | "bytes"
                | "lines"
                | "split"
                | "split_whitespace"
                | "map"
                | "filter"
                | "rev"
                | "enumerate"
                | "zip"
                | "keys"
                | "values"
                | "new"
                | "with_capacity"
                | "parse"
                | "join"
                | "concat"
                | "format"
                | "trim"
                | "len"
                | "is_empty"
        ),
        _ => true,
    }
}

/// Whether a temporary made for the expression owns a fresh value, so the statement end drops
/// it. A place read, a path, a reference and a closure hand out a handle into storage that
/// lives on, and a guard is released by `release_guard_temps`.
pub(super) fn temp_is_owned(expr: &Expr, binding_owns: BindingOwns) -> bool {
    match expr {
        Expr::Paren(p) => temp_is_owned(&p.expr, binding_owns),
        Expr::Group(g) => temp_is_owned(&g.expr, binding_owns),
        Expr::Path(_)
        | Expr::Field(_)
        | Expr::Index(_)
        | Expr::Reference(_)
        | Expr::Closure(_)
        | Expr::Lit(_)
        | Expr::Range(_)
        | Expr::Cast(_) => false,
        Expr::Unary(u) => {
            !matches!(u.op, syn::UnOp::Deref(_)) && temp_is_owned(&u.expr, binding_owns)
        }
        Expr::MethodCall(m) => match m.method.to_string().as_str() {
            "borrow" | "borrow_mut" | "try_borrow" | "try_borrow_mut" | "lock" => false,
            _ => init_is_owned(expr, binding_owns),
        },
        other => init_is_owned(other, binding_owns),
    }
}

/// Whether an iterator chain hands out items of its own. `into_iter` and `drain` take them out
/// of the collection, `cloned` makes fresh ones, `iter` lends handles into the collection.
/// `map` hands out what its closure returns, a handle when the closure lends, so it owns over
/// an owning receiver or with a closure that builds a fresh value. A collection receiver lends
/// too, `vec![..].last()` is the slice method. An iterator held in a binding owns what its
/// init chain owned, which the `let` recorded, see `binding_owns_items`.
pub(super) fn chain_owns_items(expr: &Expr, binding_owns: BindingOwns) -> bool {
    match expr {
        Expr::Paren(p) => chain_owns_items(&p.expr, binding_owns),
        Expr::Group(g) => chain_owns_items(&g.expr, binding_owns),
        Expr::Path(p) if p.path.segments.len() == 1 && p.qself.is_none() => {
            binding_owns(&p.path.segments[0].ident.to_string())
        }
        Expr::MethodCall(m) => match m.method.to_string().as_str() {
            "into_iter" | "into_keys" | "into_values" | "drain" | "cloned" | "copied" | "chars"
            | "bytes" | "char_indices" | "lines" | "split" | "split_whitespace" => true,
            "map" | "filter_map" | "flat_map" | "map_while" => {
                chain_owns_items(&m.receiver, binding_owns)
                    || m.args.first().is_some_and(closure_yields_fresh)
            }
            "filter" | "take" | "skip" | "rev" | "enumerate" | "step_by" | "take_while"
            | "skip_while" | "peekable" | "by_ref" | "fuse" | "inspect" | "flatten" | "scan" => {
                chain_owns_items(&m.receiver, binding_owns)
            }
            "zip" | "chain" => {
                chain_owns_items(&m.receiver, binding_owns)
                    && m.args.iter().all(|arg| chain_owns_items(arg, binding_owns))
            }
            _ => false,
        },
        Expr::Range(_) => true,
        _ => false,
    }
}

/// Whether a closure argument builds a fresh value from its parameter, `|x| x.clone()`, so
/// the chain hands out values of its own even over a lending receiver.
fn closure_yields_fresh(arg: &Expr) -> bool {
    match arg {
        Expr::Closure(c) => expr_is_fresh(&c.body),
        _ => false,
    }
}

/// A value nobody else holds. A path, a field or an index reads a handle out of live storage,
/// and a call may hand back a borrow of its argument, so only a call over fresh arguments
/// counts.
fn expr_is_fresh(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(p) => expr_is_fresh(&p.expr),
        Expr::Group(g) => expr_is_fresh(&g.expr),
        Expr::Block(b) => b.block.stmts.last().is_some_and(|stmt| match stmt {
            syn::Stmt::Expr(e, None) => expr_is_fresh(e),
            _ => false,
        }),
        // an operator yields a new scalar or a new string, a deref copies
        Expr::Lit(_) | Expr::Struct(_) | Expr::Binary(_) | Expr::Unary(_) | Expr::Cast(_) => true,
        Expr::Tuple(t) => t.elems.iter().all(expr_is_fresh),
        Expr::Call(c) => c.args.iter().all(expr_is_fresh),
        Expr::Macro(m) => m
            .mac
            .path
            .segments
            .last()
            .is_some_and(|s| matches!(s.ident.to_string().as_str(), "format" | "vec")),
        Expr::MethodCall(m) => matches!(
            m.method.to_string().as_str(),
            "clone"
                | "cloned"
                | "copied"
                | "to_vec"
                | "to_owned"
                | "to_string"
                | "collect"
                | "into"
                | "concat"
                | "repeat"
                | "join"
        ),
        _ => false,
    }
}

/// Whether a `let` init hands the binding a value of its own, so scope end drops it. A borrow
/// or an accessor that hands out a handle into other storage does not. An unknown method is
/// treated as a borrow, a missed drop is safer than a drop of storage someone else owns.
pub(super) fn init_is_owned(expr: &Expr, binding_owns: BindingOwns) -> bool {
    match expr {
        Expr::Paren(p) => init_is_owned(&p.expr, binding_owns),
        Expr::Group(g) => init_is_owned(&g.expr, binding_owns),
        Expr::Try(t) => init_is_owned(&t.expr, binding_owns),
        Expr::Await(a) => init_is_owned(&a.base, binding_owns),
        Expr::Reference(_) => false,
        Expr::MethodCall(m) => match m.method.to_string().as_str() {
            "clone" | "cloned" | "copied" | "to_vec" | "to_owned" | "to_string" | "collect"
            | "pop" | "remove" | "take" | "replace" | "swap_remove" | "split_off"
            | "into_inner" | "new" | "default" | "with_capacity" | "borrow" | "borrow_mut"
            | "try_borrow" | "try_borrow_mut" | "lock" | "concat" | "repeat" | "join"
            | "into_iter" | "into_keys" | "into_values" | "drain" | "split_at" | "insert"
            | "then_some" | "then" | "into" => true,
            "unwrap" | "expect" | "unwrap_or" | "unwrap_or_else" | "unwrap_or_default" | "ok"
            | "err" | "map" | "map_err" | "and_then" | "await" | "or" | "and" | "xor" | "zip"
            | "ok_or" | "ok_or_else" | "map_or" | "map_or_else" | "or_else" | "filter"
            | "flatten" | "take_if" | "transpose" => init_is_owned(&m.receiver, binding_owns),
            // an iterator terminal hands out an item of the chain's own only when the chain
            // owns its items
            "last" | "nth" | "next" | "next_back" | "max" | "min" | "max_by" | "min_by"
            | "max_by_key" | "min_by_key" | "fold" | "reduce" | "find" | "find_map"
            | "partition" | "unzip" => chain_owns_items(&m.receiver, binding_owns),
            _ => false,
        },
        Expr::Block(b) => b.block.stmts.last().is_some_and(|stmt| match stmt {
            syn::Stmt::Expr(e, None) => init_is_owned(e, binding_owns),
            _ => false,
        }),
        Expr::If(i) => i.then_branch.stmts.last().is_some_and(
            |stmt| matches!(stmt, syn::Stmt::Expr(e, None) if init_is_owned(e, binding_owns)),
        ),
        Expr::Match(m) => m
            .arms
            .iter()
            .any(|arm| init_is_owned(&arm.body, binding_owns)),
        _ => true,
    }
}
