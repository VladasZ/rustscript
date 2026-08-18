//! The types a program writes down about itself, read straight off the AST.
//!
//! This is not type inference. Every arm here reads a type the source stated,
//! a `let` annotation, a turbofish, a literal suffix, a cast, or a method
//! whose result type is fixed whatever it is called on. Anything else answers
//! `None` so the caller keeps its old behavior.
//!
//! Only `Default` payloads are ever built from these answers, which is what
//! `unwrap_or_default` needs: `None` carries no runtime type, so the empty
//! `Vec::<u8>::new().iter().min().unwrap_or_default()` has nowhere else to
//! learn it is a `0` and not an empty string.

use std::collections::HashMap;

use syn::{Expr, Lit};

use crate::interpreter::bytecode::ScalarTy;
use crate::interpreter::numeric::IntWidth;

/// The type facts a payload walk can read: the declared types of annotated
/// locals, the stated return scalars of the script's own functions, and the
/// closure parameters bound by the chain the walk is currently inside.
pub(super) struct TyEnv<'a> {
    locals: &'a HashMap<String, ScalarTy>,
    fn_returns: &'a HashMap<String, ScalarTy>,
    /// The parameter of the closure being walked, with the type the chain
    /// hands it. `Some` only while inside that closure's body.
    param: Option<(&'a str, &'a ScalarTy)>,
    /// The env this one was bound from, so a closure nested in a closure
    /// still reads the outer parameter.
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
            outer: None,
        }
    }

    /// This env plus one closure parameter, for walking that closure's body.
    fn with_param<'b>(&'b self, name: &'b str, ty: &'b ScalarTy) -> TyEnv<'b> {
        TyEnv {
            locals: self.locals,
            fn_returns: self.fn_returns,
            param: Some((name, ty)),
            outer: Some(self),
        }
    }

    /// The type stated for a bare name, a closure parameter in scope first
    /// and an annotated local otherwise.
    fn lookup(&self, name: &str) -> Option<&ScalarTy> {
        let mut env = Some(self);
        while let Some(current) = env {
            if let Some((param, ty)) = current.param
                && param == name
            {
                return Some(ty);
            }
            env = current.outer;
        }
        self.locals.get(name)
    }
}

/// The type a closure's single parameter holds, and the name it holds it
/// under. An annotation on the parameter states it outright, otherwise the
/// item type the chain feeds in does.
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

/// Walk a closure body with its parameter bound to the item type the chain
/// feeds in, so `values.iter().map(|v| v + 1)` reads `v` as the element type
/// rather than giving up on it.
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

/// The payload type of an expression that syntactically builds an `Option`,
/// for the cases where the source states it outright. Only a `Default` is ever
/// built from this, so a container answers with the kind of default it has
/// rather than with its element type.
///
/// This is not type inference. Every arm reads a type the program wrote down,
/// and anything else answers `None` so the caller keeps its old behavior.
pub(super) fn option_payload(expr: &Expr, env: &TyEnv) -> Option<ScalarTy> {
    match expr {
        Expr::Paren(inner) => option_payload(&inner.expr, env),
        Expr::Group(inner) => option_payload(&inner.expr, env),
        // A block answers through its tail expression.
        Expr::Block(block) => block_tail(&block.block).and_then(|e| option_payload(e, env)),
        // An if-else answers through whichever branch states its type,
        // `if c { Some(x as i16) } else { None::<i16> }` from either side.
        Expr::If(sel) => block_tail(&sel.then_branch)
            .and_then(|e| option_payload(e, env))
            .or_else(|| {
                sel.else_branch
                    .as_ref()
                    .and_then(|(_, e)| option_payload(e, env))
            }),
        Expr::Path(path) => {
            let segment = path.path.segments.last()?;
            // `None::<T>`, the payload is the turbofish.
            if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                return turbofish_scalar(Some(args));
            }
            // A bare name the program declared as `let opt: Option<T>`.
            match env.lookup(&segment.ident.to_string()) {
                Some(ScalarTy::Opt(payload)) => Some((**payload).clone()),
                _ => None,
            }
        }
        // `Some(x)`, the payload is whatever `x` is.
        Expr::Call(call) => {
            let Expr::Path(path) = &*call.func else {
                return None;
            };
            let last = path.path.segments.last()?;
            (last.ident == "Some")
                .then(|| call.args.first().and_then(|a| written_ty(a, env)))
                .flatten()
        }
        Expr::MethodCall(call) => match call.method.to_string().as_str() {
            // `flag.then_some(x)` is an `Option` of whatever `x` is.
            "then_some" => call.args.first().and_then(|a| written_ty(a, env)),
            // `text.parse::<T>()` states its payload in its own turbofish.
            "parse" => turbofish_scalar(call.turbofish.as_ref()),
            // `a.or(b)` keeps the payload both sides share, so either side
            // that states it answers for both.
            "or" => call
                .args
                .first()
                .and_then(|a| option_payload(a, env))
                .or_else(|| option_payload(&call.receiver, env)),
            // `opt.map(|v| e)` rewraps whatever `e` states, and
            // `opt.and_then(|v| e)` keeps the `Option` that `e` already is.
            // Both names also belong to iterators, where the answer is an
            // iterator and not an `Option` at all, so both arms only apply
            // once the receiver has proven itself an `Option`.
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
            // These hand the same payload through untouched. `ok` moves a
            // `Result` payload into an `Option`, the same layer.
            "clone" | "cloned" | "copied" | "take" | "as_ref" | "as_mut" | "filter" | "ok" => {
                option_payload(&call.receiver, env)
            }
            // `x.unwrap_or_default()`, `unwrap`, and `expect` peel one layer,
            // so their own payload is one layer further in than the receiver's.
            "unwrap_or_default" | "unwrap" | "expect" => {
                option_payload(&call.receiver, env)?.payload().cloned()
            }
            // `x.unwrap_or(d)` peels one layer the same way, and the fallback
            // argument states the same type when the receiver does not.
            "unwrap_or" => option_payload(&call.receiver, env)
                .and_then(|payload| payload.payload().cloned())
                .or_else(|| call.args.first().and_then(|a| option_payload(a, env))),
            // `v.get(i)`, the accessors, and the no-argument iterator
            // reductions answer an `Option` of the vec's element type.
            // `map.get(k)` answers an `Option` of the map's value type.
            "get" => element_ty(&call.receiver, env).or_else(|| map_value_ty(&call.receiver, env)),
            // `map.remove(k)` answers an `Option` of the map's value type. A
            // vec's `remove` answers its element outright, not an `Option`,
            // and a map receiver is the only kind this walk answers for.
            "remove" => map_value_ty(&call.receiver, env),
            // `ch.to_digit(radix)` answers an `Option<u32>` whatever the
            // receiver, the one char method with an `Option` payload.
            "to_digit" => Some(ScalarTy::Int(IntWidth::U32)),
            // `it.position(p)` counts items, so it is an `Option<usize>`
            // whatever the items are.
            "position" | "rposition" => Some(ScalarTy::Int(IntWidth::USize)),
            // `find` is two methods under one name. On a string it answers a
            // byte offset, on an iterator it answers an item, and the closure
            // argument is what tells them apart: only the iterator form takes
            // one, the string form takes a pattern.
            "find" | "rfind" => match call.args.first() {
                Some(Expr::Closure(_)) => element_ty(&call.receiver, env),
                _ => Some(ScalarTy::Int(IntWidth::USize)),
            },
            // The accessors and the reductions all answer one item of what
            // the receiver holds. A keyed reduction's argument only decides
            // which item, not what type it is.
            "first" | "last" | "pop" | "next" | "reduce" | "min_by_key" | "max_by_key" => {
                element_ty(&call.receiver, env)
            }
            "min" | "max" if call.args.is_empty() => element_ty(&call.receiver, env),
            // `x.checked_add(y)` answers an `Option` of the receiver's own
            // integer width.
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

/// The tail expression of a block, when the block ends in one.
fn block_tail(block: &syn::Block) -> Option<&Expr> {
    match block.stmts.last()? {
        syn::Stmt::Expr(expr, None) => Some(expr),
        _ => None,
    }
}

/// The element type of an expression that syntactically builds a `Vec`, a
/// `HashSet`, or an iterator over one, for the same narrow purpose as
/// `option_payload`, and just as literally: every arm reads a type the program
/// wrote down.
fn element_ty(expr: &Expr, env: &TyEnv) -> Option<ScalarTy> {
    match expr {
        Expr::Paren(inner) => element_ty(&inner.expr, env),
        Expr::Group(inner) => element_ty(&inner.expr, env),
        Expr::Block(block) => block_tail(&block.block).and_then(|e| element_ty(e, env)),
        // `(a..b)` iterates whatever its ends are, so either end that states
        // its own type answers for the range.
        Expr::Range(range) => range
            .start
            .as_ref()
            .and_then(|e| written_ty(e, env))
            .or_else(|| range.end.as_ref().and_then(|e| written_ty(e, env))),
        // An if-else answers through whichever branch states its element.
        Expr::If(sel) => block_tail(&sel.then_branch)
            .and_then(|e| element_ty(e, env))
            .or_else(|| {
                sel.else_branch
                    .as_ref()
                    .and_then(|(_, e)| element_ty(e, env))
            }),
        // A bare name the program declared as `let v: Vec<T>` or
        // `let s: HashSet<T>`, both of which iterate their element type.
        Expr::Path(path) => {
            let segment = path.path.segments.last()?;
            match env.lookup(&segment.ident.to_string()) {
                Some(ScalarTy::List(element) | ScalarTy::Set(element)) => Some((**element).clone()),
                _ => None,
            }
        }
        // A `vec![..]` literal states its element type through any element
        // that states its own.
        Expr::Macro(mac) if mac.mac.path.is_ident("vec") => vec_macro_element(&mac.mac, env),
        // `Vec::<T>::new()` and `HashSet::<T>::new()` state it in the
        // turbofish.
        Expr::Call(_) => match written_ty(expr, env) {
            Some(ScalarTy::List(element) | ScalarTy::Set(element)) => Some(*element),
            _ => None,
        },
        Expr::MethodCall(call) => match call.method.to_string().as_str() {
            // These pass elements through unchanged. The middle stages of a
            // chain are here too: they drop or reorder items, so whatever
            // survives is still an item of the same type.
            "iter" | "into_iter" | "iter_mut" | "cloned" | "copied" | "clone" | "to_vec"
            | "rev" | "filter" | "take" | "skip" | "take_while" | "skip_while" | "peekable"
            | "by_ref" => element_ty(&call.receiver, env),
            // `it.map(|x| e)` makes whatever `e` states its own type to be
            // the element type.
            "map" => match call.args.first() {
                Some(Expr::Closure(closure)) => {
                    in_closure(closure, element_ty(&call.receiver, env), env, written_ty)
                }
                _ => None,
            },
            // `it.filter_map(|x| e)` yields the payload of the `Option` that
            // `e` builds, one layer in from `map`.
            "filter_map" => match call.args.first() {
                Some(Expr::Closure(closure)) => in_closure(
                    closure,
                    element_ty(&call.receiver, env),
                    env,
                    option_payload,
                ),
                _ => None,
            },
            // `m.values()` iterates the map's value type. Keys are not here
            // because a `ScalarTy::Map` only carries the value side.
            "values" | "into_values" | "values_mut" => map_value_ty(&call.receiver, env),
            // A string iterates chars, and its bytes are u8.
            "chars" => Some(ScalarTy::Char),
            "bytes" => Some(ScalarTy::Int(IntWidth::U8)),
            // `it.collect::<Vec<T>>()` states its element in the turbofish.
            "collect" => match turbofish_scalar(call.turbofish.as_ref()) {
                Some(ScalarTy::List(element) | ScalarTy::Set(element)) => Some(*element),
                _ => None,
            },
            // The vec an unwrap settles on, from whichever side wrote its
            // type down.
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

/// The stated element type of a `vec![..]` literal, from the first element
/// that states one. The repeat form `vec![x; n]` answers through `x`.
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

/// A method whose answer has its receiver's own type. `clone` hands it
/// through untouched, the ASCII case methods keep char as char and u8 as u8,
/// and the arithmetic methods keep their receiver's width, which is how
/// `(x as u8).saturating_mul(y)` in a map closure states a u8 element.
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
            | "pow"
            | "powi"
            | "powf"
            | "abs"
            | "signum"
            | "isqrt"
    )
}

/// The type an expression states about itself, for the same narrow purpose.
pub(super) fn written_ty(expr: &Expr, env: &TyEnv) -> Option<ScalarTy> {
    match expr {
        Expr::Paren(inner) => written_ty(&inner.expr, env),
        Expr::Group(inner) => written_ty(&inner.expr, env),
        // A block answers through its tail expression, which is how a
        // `({ let mut m: HashMap<K, V> = ...; m })` vec element states itself.
        Expr::Block(block) => block_tail(&block.block).and_then(|e| written_ty(e, env)),
        // An if-else answers through whichever branch states its type, so
        // `then_some(if flag { '9' } else { c })` knows it holds a char.
        Expr::If(sel) => block_tail(&sel.then_branch)
            .and_then(|e| written_ty(e, env))
            .or_else(|| {
                sel.else_branch
                    .as_ref()
                    .and_then(|(_, e)| written_ty(e, env))
            }),
        // `value as u8` names the type at the cast.
        Expr::Cast(cast) => ScalarTy::lower(&cast.ty),
        // Arithmetic keeps its operands' type, so either side that states it
        // answers, `(x as i8) / (y as i8)` for one. A comparison is a bool.
        Expr::Binary(bin) => {
            use syn::BinOp::{
                Add, And, BitAnd, BitOr, BitXor, Div, Eq, Ge, Gt, Le, Lt, Mul, Ne, Or, Rem, Shl,
                Shr, Sub,
            };
            match bin.op {
                Add(_) | Sub(_) | Mul(_) | Div(_) | Rem(_) | BitAnd(_) | BitOr(_) | BitXor(_) => {
                    written_ty(&bin.left, env).or_else(|| written_ty(&bin.right, env))
                }
                Shl(_) | Shr(_) => written_ty(&bin.left, env),
                Eq(_) | Ne(_) | Lt(_) | Le(_) | Gt(_) | Ge(_) | And(_) | Or(_) => {
                    Some(ScalarTy::Bool)
                }
                _ => None,
            }
        }
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
        // These methods answer in their receiver's own type, so the receiver
        // states it for the whole call.
        Expr::MethodCall(call) if keeps_receiver_ty(&call.method.to_string()) => {
            written_ty(&call.receiver, env)
        }
        // `it.collect::<T>()` states its own type in the turbofish, which is
        // how a `map(|x| ...collect::<Vec<bool>>()).min()` chain learns what
        // its default is.
        Expr::MethodCall(call)
            if call.method == "collect" && turbofish_scalar(call.turbofish.as_ref()).is_some() =>
        {
            turbofish_scalar(call.turbofish.as_ref())
        }
        // A fold answers in its init's type, which the accumulator keeps
        // through every step, so `it.fold(0u8, ..).checked_mul(..)` knows
        // its payload width even when the chain runs through a `map`.
        Expr::MethodCall(call) if call.method == "fold" => {
            call.args.first().and_then(|init| written_ty(init, env))
        }
        // An unwrap's own value is the receiver's payload, so
        // `Some('\n').unwrap_or_default()` is a char, not an Option.
        Expr::MethodCall(call)
            if matches!(
                call.method.to_string().as_str(),
                "unwrap" | "expect" | "unwrap_or" | "unwrap_or_default"
            ) && option_payload(&call.receiver, env).is_some() =>
        {
            option_payload(&call.receiver, env)
        }
        // Anything that is itself an `Option` is one layer deeper, keeping
        // what it wraps so a further unwrap can still read it.
        Expr::Call(_) | Expr::Path(_) | Expr::MethodCall(_) => {
            if let Some(payload) = option_payload(expr, env) {
                Some(ScalarTy::Opt(Box::new(payload)))
            } else if is_none_path(expr) {
                Some(ScalarTy::Opt(Box::new(ScalarTy::Other)))
            } else if let Some(element) = vec_new_element(expr) {
                Some(ScalarTy::List(Box::new(element)))
            } else if let Some(container) = container_new_ty(expr) {
                Some(container)
            } else if is_string_call(expr) {
                Some(ScalarTy::Str)
            } else if let Expr::Path(path) = expr
                && path.path.segments.len() == 1
                && let Some(declared) = env.lookup(&path.path.segments[0].ident.to_string())
            {
                // A bare name the program declared with any scalar
                // annotation, `let x: u16` included.
                Some(declared.clone())
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

/// The stated return scalar of a call to one of the script's own functions,
/// `f()` when `fn f() -> f32` says so.
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

/// A call that builds a `String` outright, `String::from(..)` or
/// `String::new()`.
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

/// The stated element type of a `Vec::<T>::new()` or `VecDeque::<T>::new()`
/// call, read from the turbofish on the container segment.
fn vec_new_element(expr: &Expr) -> Option<ScalarTy> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Expr::Path(path) = &*call.func else {
        return None;
    };
    let mut segments = path.path.segments.iter().rev();
    let last = segments.next()?;
    if last.ident != "new" {
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

/// The map or set type a `HashMap::<K, V>::new()` / `HashSet::<T>::new()`
/// call states in its own turbofish, the map twin of `vec_new_element`.
fn container_new_ty(expr: &Expr) -> Option<ScalarTy> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Expr::Path(path) = &*call.func else {
        return None;
    };
    let mut segments = path.path.segments.iter().rev();
    let last = segments.next()?;
    if last.ident != "new" {
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

/// The value type of an expression that syntactically builds a map, for the
/// `map.get(k)` payload, read as literally as `element_ty`.
fn map_value_ty(expr: &Expr, env: &TyEnv) -> Option<ScalarTy> {
    match expr {
        Expr::Paren(inner) => map_value_ty(&inner.expr, env),
        Expr::Group(inner) => map_value_ty(&inner.expr, env),
        Expr::Block(block) => block_tail(&block.block).and_then(|e| map_value_ty(e, env)),
        // A bare name the program declared as `let m: HashMap<K, V>`.
        Expr::Path(path) => {
            let segment = path.path.segments.last()?;
            match env.lookup(&segment.ident.to_string()) {
                Some(ScalarTy::Map(value)) => Some((**value).clone()),
                _ => None,
            }
        }
        // `HashMap::<K, V>::new()` states it in the turbofish.
        Expr::Call(_) => match container_new_ty(expr) {
            Some(ScalarTy::Map(value)) => Some(*value),
            _ => None,
        },
        Expr::MethodCall(call) if call.method == "clone" => map_value_ty(&call.receiver, env),
        // `it.collect::<HashMap<K, V>>()` states its value type in the
        // turbofish.
        Expr::MethodCall(call) if call.method == "collect" => {
            match turbofish_scalar(call.turbofish.as_ref()) {
                Some(ScalarTy::Map(value)) => Some(*value),
                _ => None,
            }
        }
        _ => None,
    }
}

/// A bare `None`, with or without a turbofish.
fn is_none_path(expr: &Expr) -> bool {
    matches!(expr, Expr::Path(path)
        if path.path.segments.last().is_some_and(|s| s.ident == "None"))
}

/// The first concrete scalar named by a turbofish argument list.
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
