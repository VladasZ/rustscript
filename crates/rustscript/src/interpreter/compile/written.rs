//! The types a program writes down about itself. This is not inference,
//! anything not stated answers `None`. Only `Default` payloads are built
//! from these answers, `None` carries no runtime type.

use std::collections::HashMap;

use syn::{Expr, Lit};

use crate::interpreter::bytecode::ScalarTy;
use crate::interpreter::numeric::IntWidth;

pub(super) struct TyEnv<'a> {
    locals: &'a HashMap<String, ScalarTy>,
    fn_returns: &'a HashMap<String, ScalarTy>,
    /// `Some` only while inside the closure's body.
    param: Option<(&'a str, &'a ScalarTy)>,
    /// Its own `let`s shadow everything outside, even after the compiler has
    /// left the block.
    block: Option<&'a syn::Block>,
    /// So a closure nested in a closure still reads the outer parameter.
    outer: Option<&'a TyEnv<'a>>,
}

impl<'a> TyEnv<'a> {
    pub(super) fn new(
        locals: &'a HashMap<String, ScalarTy>,
        fn_returns: &'a HashMap<String, ScalarTy>,
    ) -> Self {
        Self {
            locals,
            fn_returns,
            param: None,
            block: None,
            outer: None,
        }
    }

    fn with_block<'b>(&'b self, block: &'b syn::Block) -> TyEnv<'b> {
        TyEnv {
            locals: self.locals,
            fn_returns: self.fn_returns,
            param: None,
            block: Some(block),
            outer: Some(self),
        }
    }

    fn with_param<'b>(&'b self, name: &'b str, ty: &'b ScalarTy) -> TyEnv<'b> {
        TyEnv {
            locals: self.locals,
            fn_returns: self.fn_returns,
            param: Some((name, ty)),
            block: None,
            outer: Some(self),
        }
    }

    /// Closure parameters and block `let`s innermost first, annotated locals
    /// otherwise. An unannotated block `let` answers through its init.
    fn lookup(&self, name: &str) -> Option<ScalarTy> {
        let mut env = Some(self);
        while let Some(current) = env {
            if let Some((param, ty)) = current.param
                && param == name
            {
                return Some(ty.clone());
            }
            if let Some(block) = current.block
                && let Some(local) = block_let_named(block, name)
            {
                return if let syn::Pat::Type(t) = &local.pat {
                    ScalarTy::lower(&t.ty)
                } else {
                    let init = local.init.as_ref()?;
                    written_ty(&init.expr, current.outer?)
                };
            }
            env = current.outer;
        }
        self.locals.get(name).cloned()
    }
}

/// An annotation states it, otherwise the item type the chain feeds in.
fn closure_param(closure: &syn::ExprClosure, item: Option<ScalarTy>) -> Option<(String, ScalarTy)> {
    let mut pattern = closure.inputs.first()?;
    let mut stated = None;
    if let syn::Pat::Type(typed) = pattern {
        stated = ScalarTy::lower(&typed.ty);
        pattern = &typed.pat;
    }
    match pattern {
        syn::Pat::Ident(id) => Some((id.ident.to_string(), stated.or(item)?)),
        _ => None,
    }
}

/// So `values.iter().map(|v| v + 1)` reads `v` as the element type.
fn in_closure<T>(
    closure: &syn::ExprClosure,
    item: Option<ScalarTy>,
    env: &TyEnv,
    walk: impl Fn(&Expr, &TyEnv) -> Option<T>,
) -> Option<T> {
    match closure_param(closure, item) {
        Some((name, ty)) => walk(&closure.body, &env.with_param(&name, &ty)),
        None => walk(&closure.body, env),
    }
}

/// The payload of `<Option<T>>::default()` or `Option::<T>::default()`.
fn default_call_payload(path: &syn::ExprPath) -> Option<ScalarTy> {
    let option_ty = match &path.qself {
        Some(qself) => match &*qself.ty {
            syn::Type::Path(tp) => tp.path.segments.last().cloned(),
            _ => None,
        },
        None => path.path.segments.iter().rev().nth(1).cloned(),
    }?;
    if option_ty.ident != "Option" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &option_ty.arguments else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        syn::GenericArgument::Type(ty) => ScalarTy::lower(ty),
        _ => None,
    })
}

pub(super) fn option_payload(expr: &Expr, env: &TyEnv) -> Option<ScalarTy> {
    match expr {
        Expr::Paren(inner) => option_payload(&inner.expr, env),
        Expr::Group(inner) => option_payload(&inner.expr, env),
        Expr::Block(block) => block_value(&block.block, env, option_payload),
        // Either branch.
        Expr::If(sel) => block_tail(&sel.then_branch)
            .and_then(|e| option_payload(e, env))
            .or_else(|| {
                sel.else_branch
                    .as_ref()
                    .and_then(|(_, e)| option_payload(e, env))
            }),
        // Any arm.
        Expr::Match(m) => m.arms.iter().find_map(|arm| option_payload(&arm.body, env)),
        Expr::Path(path) => {
            let segment = path.path.segments.last()?;
            // `None::<T>`.
            if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                return turbofish_scalar(Some(args));
            }
            // `let opt: Option<T>`.
            match env.lookup(&segment.ident.to_string()) {
                Some(ScalarTy::Opt(payload)) => Some(*payload),
                _ => None,
            }
        }
        // `Some(x)` and `Option::<T>::default()`.
        Expr::Call(call) => {
            let Expr::Path(path) = &*call.func else {
                return None;
            };
            let last = path.path.segments.last()?;
            if last.ident == "default" {
                return default_call_payload(path);
            }
            (last.ident == "Some")
                .then(|| call.args.first().and_then(|a| written_ty(a, env)))
                .flatten()
        }
        Expr::MethodCall(call) => match call.method.to_string().as_str() {
            // `then_some(x)`.
            "then_some" => call.args.first().and_then(|a| written_ty(a, env)),
            // `parse::<T>()`.
            "parse" => turbofish_scalar(call.turbofish.as_ref()),
            // `or` and `xor` keep the payload both sides share.
            "or" | "xor" => call
                .args
                .first()
                .and_then(|a| option_payload(a, env))
                .or_else(|| option_payload(&call.receiver, env)),
            // `a.and(b)` answers `b`'s `Option`, the receiver only when the
            // argument does not state it.
            "and" => call
                .args
                .first()
                .and_then(|a| option_payload(a, env))
                .or_else(|| option_payload(&call.receiver, env)),
            // `map` and `and_then` also belong to iterators, so both arms only
            // apply once the receiver has proven itself an `Option`.
            "map" | "and_then" => {
                let payload = option_payload(&call.receiver, env)?;
                match call.args.first() {
                    Some(Expr::Closure(closure)) if call.method == "map" => {
                        in_closure(closure, Some(payload), env, written_ty)
                    }
                    Some(Expr::Closure(closure)) => {
                        in_closure(closure, Some(payload), env, option_payload)
                    }
                    _ => None,
                }
            }
            // Same payload through, `ok` included.
            "clone" | "cloned" | "copied" | "take" | "as_ref" | "as_mut" | "filter" | "ok" => {
                option_payload(&call.receiver, env)
            }
            // Peel one layer.
            "unwrap_or_default" | "unwrap" | "expect" => {
                option_payload(&call.receiver, env)?.payload().cloned()
            }
            // `unwrap_or(d)` peels one layer, `d` states the type when the
            // receiver does not.
            "unwrap_or" => option_payload(&call.receiver, env)
                .and_then(|payload| payload.payload().cloned())
                .or_else(|| call.args.first().and_then(|a| option_payload(a, env))),
            // `v.get(i)` and the reductions answer the element, `map.get(k)`
            // the value.
            "get" => element_ty(&call.receiver, env).or_else(|| map_value_ty(&call.receiver, env)),
            // A vec's `remove` answers its element outright, so only a map
            // receiver answers here.
            "remove" => map_value_ty(&call.receiver, env),
            // `to_digit` is `Option<u32>`.
            "to_digit" => Some(ScalarTy::Int(IntWidth::U32)),
            // `position` is `Option<usize>`.
            "position" | "rposition" => Some(ScalarTy::Int(IntWidth::USize)),
            // `find` on a string answers a byte offset, on an iterator an
            // item. Only the iterator form takes a closure.
            "find" | "rfind" => match call.args.first() {
                Some(Expr::Closure(_)) => element_ty(&call.receiver, env),
                _ => Some(ScalarTy::Int(IntWidth::USize)),
            },
            // One item of what the receiver holds.
            "first" | "last" | "next_back" | "pop" | "next" | "nth" | "reduce" | "min_by_key"
            | "max_by_key" => element_ty(&call.receiver, env),
            "min" | "max" if call.args.is_empty() => element_ty(&call.receiver, env),
            // `checked_add` answers the receiver's width.
            "checked_add" | "checked_sub" | "checked_mul" | "checked_div" | "checked_rem"
            | "checked_neg" | "checked_abs" | "checked_pow" | "checked_shl" | "checked_shr"
            | "checked_div_euclid" | "checked_rem_euclid" => {
                match written_ty(&call.receiver, env) {
                    Some(ty @ ScalarTy::Int(_)) => Some(ty),
                    _ => None,
                }
            }
            _ => None,
        },
        _ => None,
    }
}

/// A tail naming a block local answers through that `let`, so 2 blocks
/// reusing a name never read each other's type.
fn block_value(
    block: &syn::Block,
    env: &TyEnv,
    read: fn(&Expr, &TyEnv) -> Option<ScalarTy>,
) -> Option<ScalarTy> {
    let tail = block_tail(block)?;
    let inner = env.with_block(block);
    read(tail, &inner)
}

/// The last one when the name is declared twice.
pub(super) fn block_let<'b>(block: &'b syn::Block, tail: &Expr) -> Option<&'b syn::Local> {
    let Expr::Path(path) = tail else {
        return None;
    };
    block_let_named(block, &path.path.get_ident()?.to_string())
}

/// The last one when the name is declared twice.
fn block_let_named<'b>(block: &'b syn::Block, name: &str) -> Option<&'b syn::Local> {
    block.stmts.iter().rev().find_map(|stmt| match stmt {
        syn::Stmt::Local(local) => {
            let pat = match &local.pat {
                syn::Pat::Type(t) => &*t.pat,
                other => other,
            };
            matches!(pat, syn::Pat::Ident(id) if id.ident == name).then_some(local)
        }
        _ => None,
    })
}

pub(super) fn block_tail(block: &syn::Block) -> Option<&Expr> {
    match block.stmts.last()? {
        syn::Stmt::Expr(expr, None) => Some(expr),
        _ => None,
    }
}

/// The element type of a `Vec`, a `HashSet` or an iterator, as literal as
/// `option_payload`.
fn element_ty(expr: &Expr, env: &TyEnv) -> Option<ScalarTy> {
    match expr {
        Expr::Paren(inner) => element_ty(&inner.expr, env),
        Expr::Group(inner) => element_ty(&inner.expr, env),
        Expr::Block(block) => block_value(&block.block, env, element_ty),
        // Either end of a range.
        Expr::Range(range) => range
            .start
            .as_ref()
            .and_then(|e| written_ty(e, env))
            .or_else(|| range.end.as_ref().and_then(|e| written_ty(e, env))),
        // Either branch.
        Expr::If(sel) => block_tail(&sel.then_branch)
            .and_then(|e| element_ty(e, env))
            .or_else(|| {
                sel.else_branch
                    .as_ref()
                    .and_then(|(_, e)| element_ty(e, env))
            }),
        Expr::Match(m) => m.arms.iter().find_map(|arm| element_ty(&arm.body, env)),
        // `let v: Vec<T>` or `let s: HashSet<T>`.
        Expr::Path(path) => {
            let segment = path.path.segments.last()?;
            match env.lookup(&segment.ident.to_string()) {
                Some(ScalarTy::List(element) | ScalarTy::Set(element)) => Some(*element),
                _ => None,
            }
        }
        // Any element of a `vec![..]`.
        Expr::Macro(mac) if mac.mac.path.is_ident("vec") => vec_macro_element(&mac.mac, env),
        // `Vec::<T>::new()` and `HashSet::<T>::new()`.
        Expr::Call(_) => match written_ty(expr, env) {
            Some(ScalarTy::List(element) | ScalarTy::Set(element)) => Some(*element),
            _ => None,
        },
        Expr::MethodCall(call) => match call.method.to_string().as_str() {
            // Elements through unchanged, the middle of a chain too.
            "iter" | "into_iter" | "iter_mut" | "cloned" | "copied" | "clone" | "to_vec"
            | "rev" | "filter" | "take" | "skip" | "take_while" | "skip_while" | "peekable"
            | "by_ref" => element_ty(&call.receiver, env),
            // `concat` flattens one layer. Without this a second `concat` fell
            // back to joining strings.
            "concat" => match written_ty(expr, env) {
                Some(ScalarTy::List(element)) => Some(*element),
                _ => None,
            },
            // `map(|x| e)` makes `e`'s type the element.
            "map" => match call.args.first() {
                Some(Expr::Closure(closure)) => {
                    in_closure(closure, element_ty(&call.receiver, env), env, written_ty)
                }
                _ => None,
            },
            // `filter_map` yields the payload of `e`'s `Option`.
            "filter_map" => match call.args.first() {
                Some(Expr::Closure(closure)) => in_closure(
                    closure,
                    element_ty(&call.receiver, env),
                    env,
                    option_payload,
                ),
                _ => None,
            },
            // Keys are not here because `ScalarTy::Map` only carries the value
            // side.
            "values" | "into_values" | "values_mut" => map_value_ty(&call.receiver, env),
            "chars" => Some(ScalarTy::Char),
            "bytes" => Some(ScalarTy::Int(IntWidth::U8)),
            // `collect::<Vec<T>>()`.
            "collect" => match turbofish_scalar(call.turbofish.as_ref()) {
                Some(ScalarTy::List(element) | ScalarTy::Set(element)) => Some(*element),
                _ => None,
            },
            // The vec an unwrap settles on.
            "unwrap" | "unwrap_or" | "unwrap_or_default" => {
                let from_receiver = match option_payload(&call.receiver, env) {
                    Some(ScalarTy::List(element)) => Some(*element),
                    _ => None,
                };
                from_receiver.or_else(|| call.args.first().and_then(|a| element_ty(a, env)))
            }
            _ => None,
        },
        _ => None,
    }
}

/// For callers outside this module.
pub(super) fn element_of(expr: &Expr, env: &TyEnv) -> Option<ScalarTy> {
    element_ty(expr, env)
}

/// The first element that states a type, or `x` in `vec![x; n]`.
fn vec_macro_element(mac: &syn::Macro, env: &TyEnv) -> Option<ScalarTy> {
    use syn::Token;
    use syn::punctuated::Punctuated;
    if let Ok(elements) = mac.parse_body_with(Punctuated::<Expr, Token![,]>::parse_terminated) {
        return elements.iter().find_map(|e| written_ty(e, env));
    }
    mac.parse_body_with(Punctuated::<Expr, Token![;]>::parse_terminated)
        .ok()?
        .first()
        .and_then(|e| written_ty(e, env))
}

/// A method whose answer has its receiver's type, `clone`, the ASCII case
/// methods and arithmetic.
fn keeps_receiver_ty(method: &str) -> bool {
    matches!(
        method,
        "clone"
            | "to_ascii_lowercase"
            | "to_ascii_uppercase"
            | "saturating_add"
            | "saturating_sub"
            | "saturating_mul"
            | "wrapping_add"
            | "wrapping_sub"
            | "wrapping_mul"
            | "rotate_left"
            | "rotate_right"
            | "rem_euclid"
            | "div_euclid"
            | "midpoint"
            | "pow"
            | "powi"
            | "powf"
            | "abs"
            | "signum"
            | "isqrt"
    )
}

pub(super) fn written_ty(expr: &Expr, env: &TyEnv) -> Option<ScalarTy> {
    match expr {
        Expr::Paren(inner) => written_ty(&inner.expr, env),
        Expr::Group(inner) => written_ty(&inner.expr, env),
        // A block answers through its tail.
        Expr::Block(block) => block_value(&block.block, env, written_ty),
        // Either branch.
        Expr::If(sel) => block_tail(&sel.then_branch)
            .and_then(|e| written_ty(e, env))
            .or_else(|| {
                sel.else_branch
                    .as_ref()
                    .and_then(|(_, e)| written_ty(e, env))
            }),
        Expr::Match(m) => m.arms.iter().find_map(|arm| written_ty(&arm.body, env)),
        Expr::Cast(cast) => ScalarTy::lower(&cast.ty),
        Expr::Binary(bin) => binary_written_ty(bin, env),
        Expr::Unary(un) => match un.op {
            syn::UnOp::Neg(_) | syn::UnOp::Not(_) => written_ty(&un.expr, env),
            _ => None,
        },
        Expr::Lit(lit) => match &lit.lit {
            Lit::Str(_) => Some(ScalarTy::Str),
            Lit::Bool(_) => Some(ScalarTy::Bool),
            Lit::Char(_) => Some(ScalarTy::Char),
            Lit::Int(int) => IntWidth::parse(int.suffix()).map(ScalarTy::Int),
            Lit::Float(float) => match float.suffix() {
                "f32" => Some(ScalarTy::F32),
                "f64" => Some(ScalarTy::F64),
                _ => None,
            },
            _ => None,
        },
        // The receiver states it for the whole call.
        Expr::MethodCall(call) if keeps_receiver_ty(&call.method.to_string()) => {
            written_ty(&call.receiver, env)
        }
        // `collect::<T>()`.
        Expr::MethodCall(call)
            if call.method == "collect" && turbofish_scalar(call.turbofish.as_ref()).is_some() =>
        {
            turbofish_scalar(call.turbofish.as_ref())
        }
        // `sum::<T>()` and `product::<T>()`.
        Expr::MethodCall(call)
            if matches!(call.method.to_string().as_str(), "sum" | "product")
                && turbofish_scalar(call.turbofish.as_ref()).is_some() =>
        {
            turbofish_scalar(call.turbofish.as_ref())
        }
        Expr::MethodCall(call) if call.method == "concat" => {
            match element_ty(&call.receiver, env) {
                Some(ScalarTy::List(inner)) => Some(ScalarTy::List(inner)),
                Some(ScalarTy::Str) => Some(ScalarTy::Str),
                _ => None,
            }
        }
        // A fold answers in its init's type.
        Expr::MethodCall(call) if call.method == "fold" => {
            call.args.first().and_then(|init| written_ty(init, env))
        }
        // An unwrap's value is the receiver's payload.
        Expr::MethodCall(call)
            if matches!(
                call.method.to_string().as_str(),
                "unwrap" | "expect" | "unwrap_or" | "unwrap_or_default"
            ) && option_payload(&call.receiver, env).is_some() =>
        {
            option_payload(&call.receiver, env)
        }
        // An `Option` is one layer deeper.
        Expr::Call(_) | Expr::Path(_) | Expr::MethodCall(_) => {
            if let Some(payload) = option_payload(expr, env) {
                Some(ScalarTy::Opt(Box::new(payload)))
            } else if is_none_path(expr) {
                Some(ScalarTy::Opt(Box::new(ScalarTy::Other)))
            } else if let Some(element) = vec_new_element(expr) {
                Some(ScalarTy::List(Box::new(element)))
            } else if let Some(container) = container_new_ty(expr) {
                Some(container)
            } else if let Some(qualified) = qualified_ctor_ty(expr) {
                Some(qualified)
            } else if is_string_call(expr) {
                Some(ScalarTy::Str)
            } else if let Expr::Path(path) = expr
                && path.path.segments.len() == 1
                && let Some(declared) = env.lookup(&path.path.segments[0].ident.to_string())
            {
                // Any scalar annotation, `let x: u16` included.
                Some(declared)
            } else {
                fn_return_ty(expr, env)
            }
        }
        Expr::Macro(mac) if mac.mac.path.is_ident("vec") => Some(ScalarTy::List(Box::new(
            vec_macro_element(&mac.mac, env).unwrap_or(ScalarTy::Other),
        ))),
        _ => None,
    }
}

/// Either arithmetic side that states its type answers. A comparison is a
/// bool.
fn binary_written_ty(bin: &syn::ExprBinary, env: &TyEnv) -> Option<ScalarTy> {
    use syn::BinOp::{
        Add, And, BitAnd, BitOr, BitXor, Div, Eq, Ge, Gt, Le, Lt, Mul, Ne, Or, Rem, Shl, Shr, Sub,
    };
    match bin.op {
        Add(_) | Sub(_) | Mul(_) | Div(_) | Rem(_) | BitAnd(_) | BitOr(_) | BitXor(_) => {
            written_ty(&bin.left, env).or_else(|| written_ty(&bin.right, env))
        }
        Shl(_) | Shr(_) => written_ty(&bin.left, env),
        Eq(_) | Ne(_) | Lt(_) | Le(_) | Gt(_) | Ge(_) | And(_) | Or(_) => Some(ScalarTy::Bool),
        _ => None,
    }
}

/// `f()` is f32 when `fn f() -> f32` says so.
fn fn_return_ty(expr: &Expr, env: &TyEnv) -> Option<ScalarTy> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Expr::Path(path) = &*call.func else {
        return None;
    };
    let segment = path.path.segments.last()?;
    env.fn_returns.get(&segment.ident.to_string()).cloned()
}

/// `String::from(..)` or `String::new()`.
fn is_string_call(expr: &Expr) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    let Expr::Path(path) = &*call.func else {
        return false;
    };
    let mut segments = path.path.segments.iter().rev();
    let is_ctor = segments
        .next()
        .is_some_and(|s| s.ident == "from" || s.ident == "new");
    is_ctor && segments.next().is_some_and(|s| s.ident == "String")
}

/// `Vec::<T>::new()` or `VecDeque::<T>::new()`.
fn vec_new_element(expr: &Expr) -> Option<ScalarTy> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Expr::Path(path) = &*call.func else {
        return None;
    };
    let mut segments = path.path.segments.iter().rev();
    let last = segments.next()?;
    if !is_ctor(&last.ident) {
        return None;
    }
    let container = segments.next()?;
    if container.ident != "Vec" && container.ident != "VecDeque" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &container.arguments else {
        return None;
    };
    turbofish_scalar(Some(args))
}

/// The map twin of `vec_new_element`.
fn container_new_ty(expr: &Expr) -> Option<ScalarTy> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Expr::Path(path) = &*call.func else {
        return None;
    };
    let mut segments = path.path.segments.iter().rev();
    let last = segments.next()?;
    if !is_ctor(&last.ident) {
        return None;
    }
    let container = segments.next()?;
    let name = container.ident.to_string();
    if !matches!(
        name.as_str(),
        "HashMap" | "BTreeMap" | "HashSet" | "BTreeSet"
    ) {
        return None;
    }
    ScalarTy::lower_segment(container)
}

fn is_ctor(name: &syn::Ident) -> bool {
    name == "new" || name == "default" || name == "with_capacity"
}

/// `<Vec<Vec<f64>>>::default()`.
fn qualified_ctor_ty(expr: &Expr) -> Option<ScalarTy> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Expr::Path(path) = &*call.func else {
        return None;
    };
    let qself = path.qself.as_ref()?;
    if !is_ctor(&path.path.segments.last()?.ident) {
        return None;
    }
    ScalarTy::lower(&qself.ty)
}

/// The value type of a map, as literal as `element_ty`.
fn map_value_ty(expr: &Expr, env: &TyEnv) -> Option<ScalarTy> {
    match expr {
        Expr::Paren(inner) => map_value_ty(&inner.expr, env),
        Expr::Group(inner) => map_value_ty(&inner.expr, env),
        Expr::Block(block) => block_value(&block.block, env, map_value_ty),
        Expr::If(sel) => block_tail(&sel.then_branch)
            .and_then(|e| map_value_ty(e, env))
            .or_else(|| {
                sel.else_branch
                    .as_ref()
                    .and_then(|(_, e)| map_value_ty(e, env))
            }),
        Expr::Match(m) => m.arms.iter().find_map(|arm| map_value_ty(&arm.body, env)),
        // `let m: HashMap<K, V>`.
        Expr::Path(path) => {
            let segment = path.path.segments.last()?;
            match env.lookup(&segment.ident.to_string()) {
                Some(ScalarTy::Map(value)) => Some(*value),
                _ => None,
            }
        }
        // `HashMap::<K, V>::new()`.
        Expr::Call(_) => match container_new_ty(expr) {
            Some(ScalarTy::Map(value)) => Some(*value),
            _ => None,
        },
        Expr::MethodCall(call) if call.method == "clone" => map_value_ty(&call.receiver, env),
        // `collect::<HashMap<K, V>>()`.
        Expr::MethodCall(call) if call.method == "collect" => {
            match turbofish_scalar(call.turbofish.as_ref()) {
                Some(ScalarTy::Map(value)) => Some(*value),
                _ => None,
            }
        }
        _ => None,
    }
}

fn is_none_path(expr: &Expr) -> bool {
    matches!(expr, Expr::Path(path)
        if path.path.segments.last().is_some_and(|s| s.ident == "None"))
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
