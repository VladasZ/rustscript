//! The type universe. A type the generator can't name is a bug it can't find, the first model knew
//! only `i64` while the bug lived on `u8`. Every capability question is decided here from the
//! type itself.

use serde::{Deserialize, Serialize};

use crate::lang::user::UserShape;
pub use crate::lang::width::{FLOAT_WIDTHS, FloatWidth, INT_WIDTHS, IntWidth};

/// So `Vec<Vec<Vec<..>>>` terminates.
pub const MAX_TY_DEPTH: usize = 2;

/// `parse` produces them and `?` converts them through a user `From`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum StdErr {
    ParseInt,
    ParseFloat,
}

impl StdErr {
    pub fn rust(self) -> &'static str {
        match self {
            Self::ParseInt => "std::num::ParseIntError",
            Self::ParseFloat => "std::num::ParseFloatError",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum Ty {
    Int(IntWidth),
    Float(FloatWidth),
    Bool,
    Char,
    Str,
    Vec(Box<Ty>),
    Opt(Box<Ty>),
    /// never printed raw, real Rust randomizes the order per process
    Map(Box<Ty>, Box<Ty>),
    /// same observation rule as `Map`
    Set(Box<Ty>),
    Tuple(Vec<Ty>),
    Res(Box<Ty>, Box<Ty>),
    StdErr(StdErr),
    /// the shape carries what typing needs, the bodies live on the block
    User(Box<UserShape>),
    /// `DiffTrace`, a program local struct whose `Drop` prints. Every move, copy, scope end and
    /// unwind of one becomes a line of output.
    Trace,
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
            Self::Tuple(items) => {
                let rendered: Vec<String> = items.iter().map(Ty::rust).collect();
                match items.len() {
                    1 => format!("({},)", rendered[0]),
                    _ => format!("({})", rendered.join(", ")),
                }
            }
            Self::Res(ok, err) => format!("Result<{}, {}>", ok.rust(), err.rust()),
            Self::StdErr(err) => err.rust().to_string(),
            Self::User(shape) => shape.name.clone(),
            Self::Trace => TRACE_NAME.to_string(),
        }
    }

    /// A non copy read renders with `.clone()` unless the generator chose a move.
    pub fn is_copy(&self) -> bool {
        match self {
            Self::Int(_) | Self::Float(_) | Self::Bool | Self::Char => true,
            // user types never derive Copy, so every read clones or moves and exercises the
            // value model
            Self::Str
            | Self::Vec(_)
            | Self::Map(..)
            | Self::Set(_)
            | Self::StdErr(_)
            | Self::User(_)
            | Self::Trace => false,
            Self::Opt(inner) => inner.is_copy(),
            Self::Tuple(items) => items.iter().all(Ty::is_copy),
            Self::Res(ok, err) => ok.is_copy() && err.is_copy(),
        }
    }

    pub fn is_int(&self) -> bool {
        matches!(self, Self::Int(_))
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, Self::Int(_) | Self::Float(_))
    }

    /// Whether `Ord` comparisons compile.
    pub fn is_ord(&self) -> bool {
        match self {
            Self::Float(_) | Self::Map(..) | Self::Set(_) | Self::StdErr(_) => false,
            Self::Int(_) | Self::Bool | Self::Char | Self::Str | Self::Trace => true,
            Self::Vec(inner) | Self::Opt(inner) => inner.is_ord(),
            Self::Tuple(items) => items.iter().all(Ty::is_ord),
            Self::Res(ok, err) => ok.is_ord() && err.is_ord(),
            Self::User(shape) => shape.derives.is_ord(),
        }
    }

    /// Full `Eq`, floats are `PartialEq` only.
    pub fn is_eq(&self) -> bool {
        match self {
            Self::Float(_) => false,
            Self::Int(_) | Self::Bool | Self::Char | Self::Str | Self::StdErr(_) | Self::Trace => {
                true
            }
            Self::Vec(inner) | Self::Opt(inner) | Self::Set(inner) => inner.is_eq(),
            Self::Map(key, value) | Self::Res(key, value) => key.is_eq() && value.is_eq(),
            Self::Tuple(items) => items.iter().all(Ty::is_eq),
            Self::User(shape) => shape.derives.is_eq(),
        }
    }

    /// A trace never hashes. A hashed container clones, iterates and drops its entries in an
    /// order real Rust randomizes per process, and every one of those prints.
    pub fn is_hash(&self) -> bool {
        match self {
            Self::Float(_) | Self::Map(..) | Self::Set(_) | Self::StdErr(_) | Self::Trace => false,
            Self::Int(_) | Self::Bool | Self::Char | Self::Str => true,
            Self::Vec(inner) | Self::Opt(inner) => inner.is_hash(),
            Self::Tuple(items) => items.iter().all(Ty::is_hash),
            Self::Res(ok, err) => ok.is_hash() && err.is_hash(),
            Self::User(shape) => shape.derives.hash,
        }
    }

    /// Hashable, `Eq` and ordered, because every observation sorts first.
    pub fn is_key(&self) -> bool {
        self.is_hash() && self.is_eq() && self.is_ord()
    }

    pub fn has_default(&self) -> bool {
        match self {
            Self::Res(..) | Self::StdErr(_) => false,
            Self::Int(_)
            | Self::Float(_)
            | Self::Bool
            | Self::Char
            | Self::Str
            | Self::Vec(_)
            | Self::Opt(_)
            | Self::Map(..)
            | Self::Set(_)
            | Self::Trace => true,
            Self::Tuple(items) => items.iter().all(Ty::has_default),
            Self::User(shape) => shape.derives.default,
        }
    }

    /// Containers and tuples are `Debug` only.
    pub fn has_display(&self) -> bool {
        match self {
            Self::Int(_)
            | Self::Float(_)
            | Self::Bool
            | Self::Char
            | Self::Str
            | Self::StdErr(_) => true,
            Self::User(shape) => shape.display,
            _ => false,
        }
    }

    pub fn elem(&self) -> Option<&Ty> {
        match self {
            Self::Vec(inner) | Self::Opt(inner) | Self::Set(inner) => Some(inner),
            _ => None,
        }
    }

    pub fn key_val(&self) -> Option<(&Ty, &Ty)> {
        match self {
            Self::Map(key, value) => Some((key, value)),
            _ => None,
        }
    }

    pub fn ok_err(&self) -> Option<(&Ty, &Ty)> {
        match self {
            Self::Res(ok, err) => Some((ok, err)),
            _ => None,
        }
    }

    pub fn depth(&self) -> usize {
        match self {
            Self::Vec(inner) | Self::Opt(inner) | Self::Set(inner) => 1 + inner.depth(),
            Self::Map(key, value) | Self::Res(key, value) => 1 + key.depth().max(value.depth()),
            Self::Tuple(items) => 1 + items.iter().map(Ty::depth).max().unwrap_or(0),
            Self::User(shape) => 1 + shape.depth,
            _ => 0,
        }
    }

    /// A float has no total order and rounds per operation order.
    pub fn contains_float(&self) -> bool {
        match self {
            Self::Float(_) => true,
            Self::Vec(inner) | Self::Opt(inner) | Self::Set(inner) => inner.contains_float(),
            Self::Map(key, value) | Self::Res(key, value) => {
                key.contains_float() || value.contains_float()
            }
            Self::Tuple(items) => items.iter().any(Ty::contains_float),
            Self::User(shape) => shape.holds.float,
            _ => false,
        }
    }

    /// Whether a value of this type prints when it drops or moves. See `is_hash` for why a map
    /// or a set never holds one.
    pub fn contains_trace(&self) -> bool {
        match self {
            Self::Trace => true,
            Self::Vec(inner) | Self::Opt(inner) | Self::Set(inner) => inner.contains_trace(),
            Self::Map(key, value) | Self::Res(key, value) => {
                key.contains_trace() || value.contains_trace()
            }
            Self::Tuple(items) => items.iter().any(Ty::contains_trace),
            Self::User(shape) => shape.holds.trace,
            _ => false,
        }
    }

    /// For coverage features and shrink signatures.
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
            Self::Tuple(_) => "lang-ty-tuple",
            Self::Res(..) => "lang-ty-result",
            Self::StdErr(_) => "lang-ty-stderr",
            Self::User(shape) if shape.is_enum() => "lang-ty-enum",
            Self::User(_) => "lang-ty-struct",
            Self::Trace => "lang-ty-trace",
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

    pub fn res_of(ok: Ty, err: Ty) -> Self {
        Self::Res(Box::new(ok), Box::new(err))
    }

    pub fn user(shape: UserShape) -> Self {
        Self::User(Box::new(shape))
    }

    pub const USIZE: Ty = Ty::Int(IntWidth::USize);
    pub const U32: Ty = Ty::Int(IntWidth::U32);
    pub const I32: Ty = Ty::Int(IntWidth::I32);
    pub const I64: Ty = Ty::Int(IntWidth::I64);
    pub const F64: Ty = Ty::Float(FloatWidth::F64);
}

pub const TRACE_NAME: &str = "DiffTrace";

/// Emitted once per program that names the type. `Clone` is derived so a clone stays silent and
/// only drops print, otherwise every observation would drown in clone lines.
pub const TRACE_DEF: &str = r#"#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
struct DiffTrace(i64);

impl Drop for DiffTrace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

"#;

/// Fixed order so generation stays deterministic per seed.
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
