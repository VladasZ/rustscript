//! The collection a `collect` lands in, from its turbofish or its inferred result.

use super::infer;
use crate::interpreter::bytecode::DefaultIr;

/// Where the source states a `collect` target, the call is renamed to a target specific method.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CollectTarget {
    Str,
    /// the flag is a `BTreeMap`
    Map(bool),
    /// the flag is a `BTreeSet`
    Set(bool),
    /// `Result<C, E>`, `None` when the turbofish leaves `C` to inference
    Result(Option<CollectInner>),
    /// `Option<C>`
    Option(Option<CollectInner>),
}

/// The `C` inside a collected `Result<C, E>` or `Option<C>`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CollectInner {
    Vec,
    Str,
    Map(bool),
    Set(bool),
    /// `collect::<Result<(), E>>()` only checks every item
    Unit,
}

impl CollectInner {
    /// The empty `C`, which is how the runtime learns what to build.
    pub(super) fn empty(self) -> DefaultIr {
        match self {
            Self::Vec => DefaultIr::Vec,
            Self::Str => DefaultIr::Str,
            Self::Map(sorted) => DefaultIr::Map(sorted),
            Self::Set(sorted) => DefaultIr::Set(sorted),
            Self::Unit => DefaultIr::Unit,
        }
    }

    fn of_type(ty: &syn::Type) -> Option<Self> {
        match ty {
            syn::Type::Tuple(t) if t.elems.is_empty() => Some(Self::Unit),
            syn::Type::Path(p) => match p.path.segments.last()?.ident.to_string().as_str() {
                "Vec" | "VecDeque" => Some(Self::Vec),
                "String" => Some(Self::Str),
                "HashMap" => Some(Self::Map(false)),
                "BTreeMap" => Some(Self::Map(true)),
                "HashSet" => Some(Self::Set(false)),
                "BTreeSet" => Some(Self::Set(true)),
                _ => None,
            },
            _ => None,
        }
    }

    pub(super) fn of_ty(ty: &infer::Ty) -> Option<Self> {
        use infer::Ty;
        Some(match ty {
            Ty::Vec(_) => Self::Vec,
            Ty::Str => Self::Str,
            Ty::Map(_, _, sorted) => Self::Map(*sorted),
            Ty::Set(_, sorted) => Self::Set(*sorted),
            Ty::Unit => Self::Unit,
            _ => return None,
        })
    }
}

impl CollectTarget {
    pub(super) fn method_name(self) -> &'static str {
        match self {
            Self::Str => "collect_string",
            Self::Map(false) => "collect_map",
            Self::Set(false) => "collect_set",
            Self::Map(true) => "collect_btree_map",
            Self::Set(true) => "collect_btree_set",
            Self::Result(_) => "collect_result",
            Self::Option(_) => "collect_option",
        }
    }

    pub(super) fn of_type(ty: &syn::Type) -> Option<Self> {
        let syn::Type::Path(p) = ty else { return None };
        let last = p.path.segments.last()?;
        let inner = || match &last.arguments {
            syn::PathArguments::AngleBracketed(args) => match args.args.first() {
                Some(syn::GenericArgument::Type(t)) => CollectInner::of_type(t),
                _ => None,
            },
            _ => None,
        };
        match last.ident.to_string().as_str() {
            "Result" => Some(Self::Result(inner())),
            "Option" => Some(Self::Option(inner())),
            "String" => Some(Self::Str),
            "HashMap" => Some(Self::Map(false)),
            "BTreeMap" => Some(Self::Map(true)),
            "HashSet" => Some(Self::Set(false)),
            "BTreeSet" => Some(Self::Set(true)),
            _ => None,
        }
    }
}
