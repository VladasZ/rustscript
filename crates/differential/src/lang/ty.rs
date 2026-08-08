//! The type universe the generator can write programs in.
//!
//! This is the piece the old model was missing. Its `Ty` was `i64`, `bool` and
//! `String`, so every generated program was width blind. `saturating_add` was
//! generated the whole time and always on `i64`, where the interpreter is
//! correct, while the bug lived on `u8`. A type the generator cannot name is a
//! bug it cannot find, so the universe here covers what the language has, not
//! what the interpreter happens to handle.

use serde::{Deserialize, Serialize};

use crate::numeric::{FloatWidth, IntWidth};

/// How deep a generated type may nest, so `Vec<Vec<Vec<..>>>` terminates.
pub const MAX_TY_DEPTH: usize = 2;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Ty {
    Int(IntWidth),
    Float(FloatWidth),
    Bool,
    Char,
    Str,
    Vec(Box<Ty>),
    Opt(Box<Ty>),
    /// `HashMap<K, V>`. Iteration order is random per process in real Rust,
    /// so a map value is only ever observed through sorted or order-neutral
    /// forms, never printed raw.
    Map(Box<Ty>, Box<Ty>),
    /// `HashSet<E>`, under the same observation rule as `Map`.
    Set(Box<Ty>),
}

impl Ty {
    pub fn rust(&self) -> String {
        match self {
            Self::Int(width) => width.rust().to_string(),
            Self::Float(width) => width.rust().to_string(),
            Self::Bool => "bool".to_string(),
            Self::Char => "char".to_string(),
            Self::Str => "String".to_string(),
            Self::Vec(inner) => format!("Vec<{}>", inner.rust()),
            Self::Opt(inner) => format!("Option<{}>", inner.rust()),
            Self::Map(key, value) => format!("HashMap<{}, {}>", key.rust(), value.rust()),
            Self::Set(elem) => format!("HashSet<{}>", elem.rust()),
        }
    }

    /// Whether a value of this type can be used twice without a clone. A
    /// non-copy value read from a binding is rendered with `.clone()`, which is
    /// what keeps generated programs free of borrow errors.
    pub fn is_copy(&self) -> bool {
        match self {
            Self::Int(_) | Self::Float(_) | Self::Bool | Self::Char => true,
            Self::Str | Self::Vec(_) | Self::Map(..) | Self::Set(_) => false,
            Self::Opt(inner) => inner.is_copy(),
        }
    }

    pub fn is_int(&self) -> bool {
        matches!(self, Self::Int(_))
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, Self::Int(_) | Self::Float(_))
    }

    /// The element type of a `Vec<E>`, `Option<E>`, or `HashSet<E>`.
    pub fn elem(&self) -> Option<&Ty> {
        match self {
            Self::Vec(inner) | Self::Opt(inner) | Self::Set(inner) => Some(inner),
            _ => None,
        }
    }

    /// The key and value types of a `HashMap<K, V>`.
    pub fn key_val(&self) -> Option<(&Ty, &Ty)> {
        match self {
            Self::Map(key, value) => Some((key, value)),
            _ => None,
        }
    }

    pub fn depth(&self) -> usize {
        match self {
            Self::Vec(inner) | Self::Opt(inner) | Self::Set(inner) => 1 + inner.depth(),
            Self::Map(key, value) => 1 + key.depth().max(value.depth()),
            _ => 0,
        }
    }

    /// A short stable tag used in coverage features and shrink signatures.
    pub fn feature(&self) -> &'static str {
        match self {
            Self::Int(IntWidth::U8) => "lang-ty-u8",
            Self::Int(IntWidth::U16) => "lang-ty-u16",
            Self::Int(IntWidth::U32) => "lang-ty-u32",
            Self::Int(IntWidth::U64) => "lang-ty-u64",
            Self::Int(IntWidth::USize) => "lang-ty-usize",
            Self::Int(IntWidth::I8) => "lang-ty-i8",
            Self::Int(IntWidth::I16) => "lang-ty-i16",
            Self::Int(IntWidth::I32) => "lang-ty-i32",
            Self::Int(IntWidth::I64) => "lang-ty-i64",
            Self::Float(FloatWidth::F32) => "lang-ty-f32",
            Self::Float(FloatWidth::F64) => "lang-ty-f64",
            Self::Bool => "lang-ty-bool",
            Self::Char => "lang-ty-char",
            Self::Str => "lang-ty-string",
            Self::Vec(_) => "lang-ty-vec",
            Self::Opt(_) => "lang-ty-option",
            Self::Map(..) => "lang-ty-map",
            Self::Set(_) => "lang-ty-set",
        }
    }

    pub fn int(width: IntWidth) -> Self {
        Self::Int(width)
    }

    pub fn vec_of(inner: Ty) -> Self {
        Self::Vec(Box::new(inner))
    }

    pub fn opt_of(inner: Ty) -> Self {
        Self::Opt(Box::new(inner))
    }

    pub fn map_of(key: Ty, value: Ty) -> Self {
        Self::Map(Box::new(key), Box::new(value))
    }

    pub fn set_of(elem: Ty) -> Self {
        Self::Set(Box::new(elem))
    }

    pub const USIZE: Ty = Ty::Int(IntWidth::USize);
    pub const U32: Ty = Ty::Int(IntWidth::U32);
    pub const I64: Ty = Ty::Int(IntWidth::I64);
}

/// The map shapes generation draws from. Wider than one entry so the key and
/// the value axis both vary, narrow enough that every combo has real script
/// precedent: counting by name, an integer index, and grouping into buckets.
pub fn map_combos() -> [(Ty, Ty); 3] {
    [
        (Ty::Str, Ty::I64),
        (Ty::I64, Ty::I64),
        (Ty::Str, Ty::vec_of(Ty::I64)),
    ]
}

/// The set shapes generation draws from.
pub fn set_elems() -> [Ty; 1] {
    [Ty::I64]
}

/// Every scalar type the generator builds bindings from, in a fixed order so
/// generation stays deterministic for a seed.
pub const SCALAR_TYPES: &[Ty] = &[
    Ty::Int(IntWidth::U8),
    Ty::Int(IntWidth::U16),
    Ty::Int(IntWidth::U32),
    Ty::Int(IntWidth::U64),
    Ty::Int(IntWidth::USize),
    Ty::Int(IntWidth::I8),
    Ty::Int(IntWidth::I16),
    Ty::Int(IntWidth::I32),
    Ty::Int(IntWidth::I64),
    Ty::Float(FloatWidth::F32),
    Ty::Float(FloatWidth::F64),
    Ty::Bool,
    Ty::Char,
    Ty::Str,
];

pub const INT_WIDTHS: &[IntWidth] = &[
    IntWidth::U8,
    IntWidth::U16,
    IntWidth::U32,
    IntWidth::U64,
    IntWidth::USize,
    IntWidth::I8,
    IntWidth::I16,
    IntWidth::I32,
    IntWidth::I64,
];

pub const FLOAT_WIDTHS: &[FloatWidth] = &[FloatWidth::F32, FloatWidth::F64];
