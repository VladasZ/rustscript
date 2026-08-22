//! Types lowered at compile time into a plain IR. syn nodes are not
//! `Send`, and this moves all name resolution to load time.

use std::sync::Arc;

use super::numeric::IntWidth;
use super::resolver::{Res, Resolver};

#[derive(Clone)]
pub enum CastIr {
    F64,
    F32,
    Char,
    Int(IntWidth),
    /// Kept so the cast fails only if it runs, dead code may hold one.
    Unsupported(Arc<str>),
}

pub fn lower_cast(ty: &syn::Type) -> CastIr {
    let name = match ty {
        syn::Type::Path(p) => p
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default(),
        _ => String::new(),
    };
    match name.as_str() {
        "f64" => CastIr::F64,
        "f32" => CastIr::F32,
        "char" => CastIr::Char,
        // u128 and i128 keep the i64 passthrough.
        _ => match IntWidth::parse(&name) {
            Some(w) => CastIr::Int(w),
            None => CastIr::Unsupported(Arc::from(name.as_str())),
        },
    }
}

/// Aliases are followed and struct paths canonicalized here, so runtime
/// never resolves a name.
#[derive(Clone)]
pub enum TypeIr {
    /// A type coercion cannot change.
    Dynamic,
    Vec(Arc<TypeIr>),
    /// Coercion leaves maps untouched, typed json uses the value type.
    MapValue(Arc<TypeIr>),
    /// Coercion turns a collected Vec into a set.
    Set(Arc<TypeIr>),
    Option(Arc<TypeIr>),
    Struct(Arc<str>),
    /// Bound by the caller's turbofish through the type environment.
    Generic(Arc<str>),
}

impl TypeIr {
    pub fn is_active(&self) -> bool {
        match self {
            TypeIr::Dynamic | TypeIr::Generic(_) | TypeIr::MapValue(_) => false,
            TypeIr::Struct(_) | TypeIr::Set(_) => true,
            TypeIr::Vec(inner) | TypeIr::Option(inner) => inner.is_active(),
        }
    }
}

/// So a `type A = B; type B = A;` cycle lowers to `Dynamic` instead of
/// hanging.
const MAX_DEPTH: u32 = 32;

/// A bare generic parameter name shadows any type of the same name.
pub fn lower_type(
    ty: &syn::Type,
    resolver: &Resolver,
    module: usize,
    generics: &[Arc<str>],
) -> TypeIr {
    lower(ty, resolver, module, generics, 0)
}

fn lower(
    ty: &syn::Type,
    resolver: &Resolver,
    module: usize,
    generics: &[Arc<str>],
    depth: u32,
) -> TypeIr {
    if depth > MAX_DEPTH {
        return TypeIr::Dynamic;
    }
    let syn::Type::Path(p) = ty else {
        return TypeIr::Dynamic;
    };
    if p.qself.is_none()
        && p.path.segments.len() == 1
        && matches!(p.path.segments[0].arguments, syn::PathArguments::None)
        && let Some(g) = generics.iter().find(|g| p.path.segments[0].ident == ***g)
    {
        return TypeIr::Generic(Arc::from(&**g));
    }
    let Some(seg) = p.path.segments.last() else {
        return TypeIr::Dynamic;
    };
    let name = seg.ident.to_string();
    let arg = |i: usize| {
        type_arg(seg, i).map(|t| Arc::new(lower(t, resolver, module, generics, depth + 1)))
    };
    match name.as_str() {
        "Vec" | "VecDeque" => arg(0).map_or(TypeIr::Dynamic, TypeIr::Vec),
        "Option" => arg(0).map_or(TypeIr::Dynamic, TypeIr::Option),
        "Box" | "Rc" | "Arc" => match type_arg(seg, 0) {
            Some(t) => lower(t, resolver, module, generics, depth + 1),
            None => TypeIr::Dynamic,
        },
        "HashMap" | "BTreeMap" => arg(1).map_or(TypeIr::Dynamic, TypeIr::MapValue),
        "HashSet" | "BTreeSet" => arg(0).map_or(TypeIr::Dynamic, TypeIr::Set),
        _ => {
            if let Some(canon) = resolver.resolve_struct_key(module, &p.path) {
                return TypeIr::Struct(Arc::from(&*canon));
            }
            let segs: Vec<String> = p
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            match resolver.resolve(module, &segs) {
                // An alias target resolves in its own module, where no function
                // generics apply.
                Ok(Res::Alias(m, target)) => lower(&target, resolver, m, &[], depth + 1),
                _ => TypeIr::Dynamic,
            }
        }
    }
}

fn type_arg(seg: &syn::PathSegment, i: usize) -> Option<&syn::Type> {
    match &seg.arguments {
        syn::PathArguments::AngleBracketed(a) => a
            .args
            .iter()
            .filter_map(|g| match g {
                syn::GenericArgument::Type(t) => Some(t),
                _ => None,
            })
            .nth(i),
        _ => None,
    }
}
