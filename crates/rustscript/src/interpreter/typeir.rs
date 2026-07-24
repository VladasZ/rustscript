//! Types lowered at compile time into a plain IR, so chunks hold no syn AST.
//! syn nodes are not `Send`, so this lowering is what lets the same cast,
//! coercion, and turbofish tables serve both engines, and it moves all name
//! resolution to load time, out of the hot runtime paths.

use std::rc::Rc;
use std::sync::Arc;

use super::numeric::IntWidth;
use super::resolver::{Res, Resolver};

/// Target of an `as` cast, reduced to what the engines act on.
#[derive(Clone)]
pub enum CastIr {
    F64,
    F32,
    Char,
    Int(IntWidth),
    /// A target with no runtime semantics. Kept so the cast fails only if it
    /// actually runs, the way it always did, since dead code may hold one.
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
        // u128 and i128 carry no runtime width yet and keep the old
        // i64-passthrough.
        "u128" | "i128" => CastIr::Int(IntWidth::I64),
        _ => match IntWidth::parse(&name) {
            Some(w) => CastIr::Int(w),
            None => CastIr::Unsupported(Arc::from(name.as_str())),
        },
    }
}

/// A type annotation or turbofish, lowered against the module it was written
/// in. Aliases are followed and struct paths canonicalized here, so runtime
/// never resolves a name.
#[derive(Clone)]
pub enum TypeIr {
    /// A type coercion cannot change, parsed dynamically by typed json.
    Dynamic,
    /// `Vec<T>` or `VecDeque<T>`.
    Vec(Arc<TypeIr>),
    /// The value type of `HashMap<K, V>` or `BTreeMap<K, V>`. Coercion leaves
    /// maps untouched, typed json uses it for the entry values.
    MapValue(Arc<TypeIr>),
    Option(Arc<TypeIr>),
    /// A user struct, by canonical name.
    Struct(Arc<str>),
    /// A generic parameter of the enclosing function, bound to a concrete
    /// type by the caller's turbofish through the type environment.
    Generic(Arc<str>),
}

impl TypeIr {
    /// Whether coercing a value through this type can ever change it.
    pub fn is_active(&self) -> bool {
        match self {
            TypeIr::Dynamic | TypeIr::Generic(_) | TypeIr::MapValue(_) => false,
            TypeIr::Struct(_) => true,
            TypeIr::Vec(inner) | TypeIr::Option(inner) => inner.is_active(),
        }
    }
}

/// Bound on alias chains, so a `type A = B; type B = A;` cycle lowers to
/// `Dynamic` instead of hanging the compiler.
const MAX_DEPTH: u32 = 32;

/// `generics` are the type parameter names of the function being compiled. A
/// bare parameter name shadows any type of the same name, as in real Rust.
pub fn lower_type(
    ty: &syn::Type,
    resolver: &Resolver,
    module: usize,
    generics: &[Rc<str>],
) -> TypeIr {
    lower(ty, resolver, module, generics, 0)
}

fn lower(
    ty: &syn::Type,
    resolver: &Resolver,
    module: usize,
    generics: &[Rc<str>],
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
        "Vec" | "VecDeque" => arg(0).map(TypeIr::Vec).unwrap_or(TypeIr::Dynamic),
        "Option" => arg(0).map(TypeIr::Option).unwrap_or(TypeIr::Dynamic),
        // Smart pointers are transparent at runtime.
        "Box" | "Rc" | "Arc" => match type_arg(seg, 0) {
            Some(t) => lower(t, resolver, module, generics, depth + 1),
            None => TypeIr::Dynamic,
        },
        "HashMap" | "BTreeMap" => arg(1).map(TypeIr::MapValue).unwrap_or(TypeIr::Dynamic),
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
                // An alias target resolves in the module the alias was
                // declared in, where no function generics apply.
                Ok(Res::Alias(m, target)) => lower(&target, resolver, m, &[], depth + 1),
                _ => TypeIr::Dynamic,
            }
        }
    }
}

/// The `i`-th type argument of a segment, `HashMap<K, V>` at 1 gives `V`.
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
