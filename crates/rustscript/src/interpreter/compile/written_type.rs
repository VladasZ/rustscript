//! The full written type of an expression, read off the source. This is not
//! inference. Anything not written down answers `None`.

use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;
use syn::punctuated::Punctuated;
use syn::{Expr, Lit, Type};

use super::Compiler;
use super::written::block_let;

impl Compiler<'_> {
    pub(super) fn written_type(&self, expr: &Expr) -> Option<Type> {
        self.written_type_in(expr, &[])
    }

    /// Innermost last, so a name resolves to its own block's `let` even after
    /// the compiler has left the block.
    fn written_type_in(&self, expr: &Expr, blocks: &[&syn::Block]) -> Option<Type> {
        match expr {
            Expr::Paren(p) => self.written_type_in(&p.expr, blocks),
            Expr::Group(g) => self.written_type_in(&g.expr, blocks),
            Expr::Reference(r) => self.written_type_in(&r.expr, blocks),
            Expr::Path(p) if p.qself.is_none() && p.path.segments.len() == 1 => {
                let seg = &p.path.segments[0];
                let name = seg.ident.to_string();
                if name == "None" {
                    return generic_arg(seg, 0).map(|t| option_of(&t));
                }
                if let Some(bound) = self.bound_param_type(&name) {
                    return Some(bound);
                }
                for (depth, block) in blocks.iter().enumerate().rev() {
                    if let Some(local) = block_let(block, expr) {
                        return match &local.pat {
                            syn::Pat::Type(t) => Some((*t.ty).clone()),
                            _ => local.init.as_ref().and_then(|init| {
                                self.written_type_in(&init.expr, &blocks[..depth])
                            }),
                        };
                    }
                }
                self.typed_local_types.get(&name).cloned()
            }
            // `Enum::Variant` states the enum it belongs to.
            Expr::Path(p) if p.qself.is_none() && p.path.segments.len() > 1 => {
                self.variant_owner_type(&p.path)
            }
            Expr::Call(call) => self.written_call_type(call, blocks),
            Expr::MethodCall(m) => self.written_method_type(m, blocks),
            Expr::Cast(c) => Some((*c.ty).clone()),
            Expr::Unary(u) if matches!(u.op, syn::UnOp::Not(_) | syn::UnOp::Neg(_)) => {
                self.written_type_in(&u.expr, blocks)
            }
            Expr::Binary(b) => self.written_binary_type(b, blocks),
            Expr::Macro(mac) if mac.mac.path.is_ident("vec") => {
                self.written_vec_macro_type(mac, blocks)
            }
            // A range index keeps the sequence itself.
            Expr::Index(ix) => {
                let base = self.written_type_in(&ix.expr, blocks)?;
                if matches!(&*ix.index, Expr::Range(_)) {
                    return Some(base);
                }
                let seg = last_segment(&base)?;
                match seg.ident.to_string().as_str() {
                    "Vec" | "VecDeque" => generic_arg(seg, 0),
                    "HashMap" | "BTreeMap" => generic_arg(seg, 1),
                    _ => None,
                }
            }
            Expr::Array(array) => array
                .elems
                .first()
                .and_then(|e| self.written_type_in(e, blocks))
                .map(|t| generic_type("Vec", vec![t])),
            Expr::Repeat(rep) => self
                .written_type_in(&rep.expr, blocks)
                .map(|t| generic_type("Vec", vec![t])),
            Expr::Range(range) => self.range_type(range, blocks),
            Expr::Struct(lit) => self.named_user_type(lit.path.clone()),
            Expr::Tuple(t) => t
                .elems
                .iter()
                .map(|e| self.written_type_in(e, blocks))
                .collect::<Option<Vec<_>>>()
                .map(tuple_type),
            Expr::Lit(lit) => match &lit.lit {
                Lit::Int(int) if !int.suffix().is_empty() => Some(named_type(int.suffix())),
                Lit::Float(float) if !float.suffix().is_empty() => Some(named_type(float.suffix())),
                Lit::Bool(_) => Some(named_type("bool")),
                Lit::Char(_) => Some(named_type("char")),
                _ => None,
            },
            // The first branch that states its type answers for all.
            Expr::If(e) => block_tail(&e.then_branch)
                .and_then(|tail| self.written_type_in(tail, blocks))
                .or_else(|| {
                    e.else_branch
                        .as_ref()
                        .and_then(|(_, other)| self.written_type_in(other, blocks))
                }),
            Expr::Match(m) => m
                .arms
                .iter()
                .find_map(|arm| self.written_type_in(&arm.body, blocks)),
            // A tail naming a block local reads that `let`, so 2 blocks reusing
            // a name never read each other's type.
            Expr::Block(b) => {
                let tail = block_tail(&b.block)?;
                let mut inner: Vec<&syn::Block> = blocks.to_vec();
                inner.push(&b.block);
                self.written_type_in(tail, &inner)
            }
            _ => None,
        }
    }

    /// Its own turbofish, a receiver independent name, or one hop from the
    /// receiver's type.
    fn written_method_type(&self, m: &syn::ExprMethodCall, blocks: &[&syn::Block]) -> Option<Type> {
        let method = m.method.to_string();
        // `parse::<T>()` is a `Result<T, _>`.
        if let Some(turbofish) = &m.turbofish
            && let Some(stated) = turbofish.args.iter().find_map(|arg| match arg {
                syn::GenericArgument::Type(ty) => Some(ty.clone()),
                _ => None,
            })
        {
            match method.as_str() {
                "collect" | "sum" | "product" => return Some(stated),
                "parse" => return Some(result_of(stated, None)),
                _ => {}
            }
        }
        // A fold answers in its init's type.
        if method == "fold"
            && let Some(init) = m.args.first()
            && let Some(ty) = self.written_type_in(init, blocks)
        {
            return Some(ty);
        }
        match method.as_str() {
            "len" | "count" | "capacity" => return Some(named_type("usize")),
            "is_empty" | "is_some" | "is_none" | "is_ok" | "is_err" | "contains"
            | "contains_key" | "starts_with" | "ends_with" | "any" | "all" => {
                return Some(named_type("bool"));
            }
            "to_string" | "to_uppercase" | "to_lowercase" | "join" | "repeat" | "replace" => {
                return Some(named_type("String"));
            }
            _ => {}
        }
        if method == "then_some" {
            return m
                .args
                .first()
                .and_then(|a| self.written_type_in(a, blocks))
                .map(|t| option_of(&t));
        }
        match self.written_type_in(&m.receiver, blocks) {
            Some(recv) => {
                hop(&recv, &method).or_else(|| self.closure_hop(&recv, &method, m, blocks))
            }
            // `unwrap_or(d)` answers in `d`'s type when the receiver does not
            // state its own.
            None if method == "unwrap_or" => {
                m.args.first().and_then(|d| self.written_type_in(d, blocks))
            }
            None => None,
        }
    }

    /// Either arithmetic side that states its type answers, a shift the
    /// shifted side alone.
    fn written_binary_type(&self, b: &syn::ExprBinary, blocks: &[&syn::Block]) -> Option<Type> {
        use syn::BinOp::{
            Add, And, BitAnd, BitOr, BitXor, Div, Eq, Ge, Gt, Le, Lt, Mul, Ne, Or, Rem, Shl, Shr,
            Sub,
        };
        match b.op {
            Eq(_) | Ne(_) | Lt(_) | Le(_) | Gt(_) | Ge(_) | And(_) | Or(_) => {
                Some(named_type("bool"))
            }
            Add(_) | Sub(_) | Mul(_) | Div(_) | Rem(_) | BitAnd(_) | BitOr(_) | BitXor(_) => self
                .written_type_in(&b.left, blocks)
                .or_else(|| self.written_type_in(&b.right, blocks)),
            Shl(_) | Shr(_) => self.written_type_in(&b.left, blocks),
            _ => None,
        }
    }

    /// The first element that states its own type, or `x` in `vec![x; n]`.
    fn written_vec_macro_type(&self, mac: &syn::ExprMacro, blocks: &[&syn::Block]) -> Option<Type> {
        let elems = mac
            .mac
            .parse_body_with(Punctuated::<Expr, syn::Token![,]>::parse_terminated)
            .ok();
        let first = match &elems {
            Some(elems) => elems.first().and_then(|e| self.written_type_in(e, blocks)),
            None => mac
                .mac
                .parse_body::<syn::ExprRepeat>()
                .ok()
                .and_then(|rep| self.written_type_in(&rep.expr, blocks)),
        };
        first.map(|t| generic_type("Vec", vec![t]))
    }

    fn written_call_type(&self, call: &syn::ExprCall, blocks: &[&syn::Block]) -> Option<Type> {
        let Expr::Path(path) = &*call.func else {
            return None;
        };
        if let Some(qself) = &path.qself {
            return Some((*qself.ty).clone());
        }
        let segs = &path.path.segments;
        let last = segs.last()?;
        // `Enum::Variant(payload)` states the enum it belongs to.
        if segs.len() > 1
            && let Some(owner) = variant_owner(&path.path)
            && self.user_type_key(&owner).is_some()
        {
            return Some(syn::Type::Path(syn::TypePath {
                attrs: Vec::new(),
                qself: None,
                path: owner,
            }));
        }
        if segs.len() == 1 {
            let name = last.ident.to_string();
            let first_arg = || {
                call.args
                    .first()
                    .and_then(|a| self.written_type_in(a, blocks))
            };
            return match name.as_str() {
                "None" => generic_arg(last, 0).map(|t| option_of(&t)),
                // `Err::<T, E>(e)` only states `T` in a turbofish.
                "Some" => generic_arg(last, 0)
                    .or_else(first_arg)
                    .map(|t| option_of(&t)),
                "Ok" => generic_arg(last, 0)
                    .or_else(first_arg)
                    .map(|ok| result_of(ok, generic_arg(last, 1))),
                "Err" => generic_arg(last, 0).map(|ok| result_of(ok, generic_arg(last, 1))),
                _ => {
                    let ret = self.ctx.fn_return_types.get(&name).cloned()?;
                    self.generic_return(&name, ret, call, blocks)
                }
            };
        }
        // `Vec::<T>` from `Vec::<T>::new()`.
        let type_path = syn::Path {
            leading_colon: path.path.leading_colon,
            segments: segs.iter().take(segs.len() - 1).cloned().collect(),
        };
        let owner = type_path.segments.last()?;
        let known = matches!(
            owner.ident.to_string().as_str(),
            "Vec"
                | "VecDeque"
                | "HashMap"
                | "BTreeMap"
                | "HashSet"
                | "BTreeSet"
                | "Option"
                | "String"
        );
        let constructor = matches!(
            last.ident.to_string().as_str(),
            "new" | "default" | "from" | "with_capacity"
        );
        if !known || !constructor {
            return None;
        }
        Some(path_type(type_path))
    }

    /// `fn pick<T>(a: T, b: T) -> T` answers in the written type of an
    /// argument passed for `T`.
    fn generic_return(
        &self,
        name: &str,
        ret: Type,
        call: &syn::ExprCall,
        blocks: &[&syn::Block],
    ) -> Option<Type> {
        let Some(param) = single_ident(&ret) else {
            return Some(ret);
        };
        let Some(sig) = self.ctx.fn_signatures.get(name) else {
            return Some(ret);
        };
        if !sig.generics.type_params().any(|p| p.ident == param) {
            return Some(ret);
        }
        sig.inputs
            .iter()
            .enumerate()
            .filter_map(|(index, input)| match input {
                syn::FnArg::Typed(t) if single_ident(&t.ty).as_deref() == Some(&param) => {
                    call.args.iter().nth(index)
                }
                _ => None,
            })
            .find_map(|arg| self.written_type_in(arg, blocks))
    }

    /// `Enum::Variant` states the enum it belongs to.
    fn variant_owner_type(&self, path: &syn::Path) -> Option<Type> {
        self.named_user_type(variant_owner(path)?)
    }

    fn named_user_type(&self, path: syn::Path) -> Option<Type> {
        self.user_type_key(&path).map(|_| {
            syn::Type::Path(syn::TypePath {
                attrs: Vec::new(),
                qself: None,
                path,
            })
        })
    }

    /// A parameter shadows anything outside it.
    fn bound_param_type(&self, name: &str) -> Option<Type> {
        self.closure_param_types.borrow().get(name).cloned()
    }

    /// `a..b` stands in for a sequence of its end type.
    fn range_type(&self, range: &syn::ExprRange, blocks: &[&syn::Block]) -> Option<Type> {
        range
            .start
            .as_ref()
            .and_then(|e| self.written_type_in(e, blocks))
            .or_else(|| {
                range
                    .end
                    .as_ref()
                    .and_then(|e| self.written_type_in(e, blocks))
            })
            .map(|t| generic_type("Vec", vec![t]))
    }

    /// Answers `None` rather than reading an outer local of the same name.
    fn closure_hop(
        &self,
        recv: &Type,
        method: &str,
        m: &syn::ExprMethodCall,
        blocks: &[&syn::Block],
    ) -> Option<Type> {
        if method != "map" || !is_sequence(recv) {
            return None;
        }
        let Some(Expr::Closure(closure)) = m.args.first() else {
            return None;
        };
        // Any pattern beyond a plain name binds names this walk does not
        // track.
        let mut params = Vec::new();
        for input in &closure.inputs {
            let pat = match input {
                syn::Pat::Type(t) => &*t.pat,
                other => other,
            };
            match pat {
                syn::Pat::Ident(id) => params.push(id.ident.to_string()),
                syn::Pat::Wild(_) => {}
                _ => return None,
            }
        }
        // Bind the element type for the walk of the body. A struct literal or
        // a cast needs no binding.
        let element = element_of_sequence(recv);
        let reads_params = mentions_any(&closure.body, &params);
        if reads_params && element.is_none() && !states_own_type(&closure.body) {
            return None;
        }
        let bound = match (&element, reads_params) {
            (Some(ty), true) => {
                let mut map = self.closure_param_types.borrow_mut();
                let saved: Vec<(String, Option<Type>)> = params
                    .iter()
                    .map(|name| (name.clone(), map.insert(name.clone(), ty.clone())))
                    .collect();
                saved
            }
            _ => Vec::new(),
        };
        let item = self.written_type_in(&closure.body, blocks);
        {
            let mut map = self.closure_param_types.borrow_mut();
            for (name, previous) in bound {
                match previous {
                    Some(ty) => map.insert(name, ty),
                    None => map.remove(&name),
                };
            }
        }
        Some(generic_type("Vec", vec![item?]))
    }
}

/// A field or method of the same name counts too, which only makes the
/// answer more cautious.
fn mentions_any(expr: &Expr, names: &[String]) -> bool {
    fn walk(tokens: TokenStream, names: &[String]) -> bool {
        tokens.into_iter().any(|tree| match tree {
            TokenTree::Ident(ident) => names.iter().any(|name| ident == name),
            TokenTree::Group(group) => walk(group.stream(), names),
            _ => false,
        })
    }
    walk(expr.to_token_stream(), names)
}

/// Names its own type without reading anything, so a closure body of this
/// shape answers even when it mentions the parameter.
fn states_own_type(expr: &Expr) -> bool {
    match expr {
        Expr::Struct(_) | Expr::Cast(_) => true,
        Expr::Paren(inner) => states_own_type(&inner.expr),
        Expr::Group(inner) => states_own_type(&inner.expr),
        _ => false,
    }
}

pub(super) fn sequence_element(ty: &Type) -> Option<Type> {
    element_of_sequence(ty)
}

fn element_of_sequence(ty: &Type) -> Option<Type> {
    let seg = last_segment(ty)?;
    match seg.ident.to_string().as_str() {
        "Vec" | "VecDeque" | "HashSet" | "BTreeSet" => generic_arg(seg, 0),
        _ => None,
    }
}

fn is_primitive_number(name: &str) -> bool {
    matches!(
        name,
        "i8" | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "f32"
            | "f64"
    )
}

fn is_sequence(ty: &Type) -> bool {
    last_segment(ty).is_some_and(|seg| {
        matches!(
            seg.ident.to_string().as_str(),
            "Vec" | "VecDeque" | "HashSet" | "BTreeSet"
        )
    })
}

/// The owner of `Enum::Variant`.
fn variant_owner(path: &syn::Path) -> Option<syn::Path> {
    if path.segments.len() < 2 {
        return None;
    }
    let mut owner = path.clone();
    let kept: Vec<syn::PathSegment> = owner
        .segments
        .iter()
        .take(path.segments.len() - 1)
        .cloned()
        .collect();
    owner.segments = kept.into_iter().collect();
    Some(owner)
}

fn block_tail(block: &syn::Block) -> Option<&Expr> {
    match block.stmts.last()? {
        syn::Stmt::Expr(expr, None) => Some(expr),
        _ => None,
    }
}

/// `Vec::<u8>::new()` gives `Vec<u8>`.
fn path_type(path: syn::Path) -> Type {
    Type::Path(syn::TypePath {
        attrs: Vec::new(),
        qself: None,
        path,
    })
}

fn generic_type(name: &str, args: Vec<Type>) -> Type {
    let ident = syn::Ident::new(name, proc_macro2::Span::call_site());
    let arguments = if args.is_empty() {
        syn::PathArguments::None
    } else {
        syn::PathArguments::AngleBracketed(syn::AngleBracketedGenericArguments {
            colon2_token: None,
            lt_token: syn::token::Lt::default(),
            args: args.into_iter().map(syn::GenericArgument::Type).collect(),
            gt_token: syn::token::Gt::default(),
        })
    };
    let mut segments = syn::punctuated::Punctuated::new();
    segments.push(syn::PathSegment { ident, arguments });
    path_type(syn::Path {
        leading_colon: None,
        segments,
    })
}

fn generic_arg(seg: &syn::PathSegment, n: usize) -> Option<Type> {
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    args.args
        .iter()
        .filter_map(|arg| match arg {
            syn::GenericArgument::Type(ty) => Some(ty.clone()),
            _ => None,
        })
        .nth(n)
}

fn last_segment(ty: &Type) -> Option<&syn::PathSegment> {
    match ty {
        Type::Path(p) => p.path.segments.last(),
        Type::Paren(p) => last_segment(&p.elem),
        Type::Group(g) => last_segment(&g.elem),
        Type::Reference(r) => last_segment(&r.elem),
        _ => None,
    }
}

fn named_type(name: &str) -> Type {
    generic_type(name, Vec::new())
}

fn tuple_type(elems: Vec<Type>) -> Type {
    Type::Tuple(syn::TypeTuple {
        attrs: Vec::new(),
        paren_token: syn::token::Paren::default(),
        elems: elems.into_iter().collect(),
    })
}

fn option_of(inner: &Type) -> Type {
    generic_type("Option", vec![inner.clone()])
}

/// `_` stands in for an inferred error type, only the payload is ever read.
fn result_of(ok: Type, err: Option<Type>) -> Type {
    let err = err.unwrap_or_else(|| {
        Type::Infer(syn::TypeInfer {
            attrs: Vec::new(),
            underscore_token: syn::token::Underscore::default(),
        })
    });
    generic_type("Result", vec![ok, err])
}

pub(super) fn payload_of(ty: &Type) -> Option<Type> {
    let seg = last_segment(ty)?;
    let name = seg.ident.to_string();
    if name == "Option" || name == "Result" {
        return generic_arg(seg, 0);
    }
    None
}

fn hop(recv: &Type, method: &str) -> Option<Type> {
    // A tuple has no segment but still hands itself through the identity
    // methods.
    match recv {
        Type::Array(syn::TypeArray { elem, .. }) | Type::Slice(syn::TypeSlice { elem, .. }) => {
            return hop(&generic_type("Vec", vec![(**elem).clone()]), method);
        }
        Type::Tuple(_) => {
            return matches!(
                method,
                "clone" | "cloned" | "copied" | "as_ref" | "as_mut" | "take" | "to_owned"
            )
            .then(|| recv.clone());
        }
        _ => {}
    }
    let seg = last_segment(recv)?;
    let name = seg.ident.to_string();
    match method {
        "clone" | "cloned" | "copied" | "as_ref" | "as_mut" | "take" | "unwrap" | "expect"
            if name != "Option" && name != "Result" =>
        {
            Some(recv.clone())
        }
        // After `values` or `keys` the chain walks a sequence of that side.
        "values" | "into_values" | "values_mut"
            if matches!(name.as_str(), "HashMap" | "BTreeMap") =>
        {
            generic_arg(seg, 1).map(|v| generic_type("Vec", vec![v]))
        }
        "keys" | "into_keys" if matches!(name.as_str(), "HashMap" | "BTreeMap") => {
            generic_arg(seg, 0).map(|k| generic_type("Vec", vec![k]))
        }
        "clone" | "cloned" | "copied" | "as_ref" | "as_mut" | "take" | "or" | "xor" | "and"
        | "filter" | "or_else" | "or_default" | "iter" | "into_iter" | "values" | "into_values" => {
            Some(recv.clone())
        }
        // `(x as u8).saturating_add(y)` states a `u8`.
        "saturating_add" | "saturating_sub" | "saturating_mul" | "wrapping_add"
        | "wrapping_sub" | "wrapping_mul" | "rem_euclid" | "div_euclid" | "midpoint" | "pow"
        | "powi" | "powf" | "abs" | "signum" | "isqrt" | "to_ascii_lowercase"
        | "to_ascii_uppercase" => Some(recv.clone()),
        // On a number `min` and `max` keep the type, on a sequence they
        // reduce to an `Option`.
        "min" | "max" | "clamp" if is_primitive_number(&name) => Some(recv.clone()),
        // The middle of a chain keeps the item type.
        "rev" | "skip" | "take_while" | "skip_while" | "peekable" | "by_ref"
            if matches!(name.as_str(), "Vec" | "VecDeque" | "HashSet" | "BTreeSet") =>
        {
            Some(recv.clone())
        }
        "unwrap" | "expect" | "unwrap_or" | "unwrap_or_default" | "unwrap_or_else" => {
            payload_of(recv)
        }
        "ok" | "err" if name == "Result" => {
            let index = usize::from(method == "err");
            generic_arg(seg, index).map(|t| option_of(&t))
        }
        "get" | "first" | "last" | "next_back" | "pop" | "iter_max" | "max" | "min" | "next"
        | "nth"
            if matches!(name.as_str(), "Vec" | "VecDeque" | "HashSet" | "BTreeSet") =>
        {
            generic_arg(seg, 0).map(|t| option_of(&t))
        }
        "get" | "remove" if matches!(name.as_str(), "HashMap" | "BTreeMap") => {
            generic_arg(seg, 1).map(|t| option_of(&t))
        }
        "concat" if matches!(name.as_str(), "Vec" | "VecDeque") => {
            let elem = generic_arg(seg, 0)?;
            match last_segment(&elem).map(|s| s.ident.to_string()).as_deref() {
                Some("Vec" | "VecDeque") => Some(elem),
                Some("String" | "str") => Some(named_type("String")),
                _ => None,
            }
        }
        "keys" | "into_keys" if matches!(name.as_str(), "HashMap" | "BTreeMap") => {
            let key = generic_arg(seg, 0)?;
            Some(generic_type("HashSet", vec![key]))
        }
        _ => None,
    }
}

fn single_ident(ty: &Type) -> Option<String> {
    let Type::Path(p) = ty else {
        return None;
    };
    if p.qself.is_some() || p.path.segments.len() != 1 {
        return None;
    }
    let seg = &p.path.segments[0];
    matches!(seg.arguments, syn::PathArguments::None).then(|| seg.ident.to_string())
}
