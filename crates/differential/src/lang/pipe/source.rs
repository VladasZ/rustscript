//! Where the items of a pipe come from, and what the chain so far can do.

use serde::{Deserialize, Serialize};

use crate::lang::expr::Expr;
use crate::lang::ty::{IntWidth, Ty};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Item {
    Scalar(Ty),
    Pair(Ty, Ty),
}

impl Item {
    pub fn rust(&self) -> String {
        match self {
            Self::Scalar(ty) => ty.rust(),
            Self::Pair(key, value) => format!("({}, {})", key.rust(), value.rust()),
        }
    }

    /// The item as one type, a pair is a tuple.
    pub fn ty(&self) -> Ty {
        match self {
            Self::Scalar(ty) => ty.clone(),
            Self::Pair(key, value) => Ty::Tuple(vec![key.clone(), value.clone()]),
        }
    }

    /// The 2 halves of a pair item, also when it travels as a tuple.
    pub fn halves(&self) -> Option<(Ty, Ty)> {
        match self {
            Self::Pair(key, value) => Some((key.clone(), value.clone())),
            Self::Scalar(Ty::Tuple(parts)) if parts.len() == 2 => {
                Some((parts[0].clone(), parts[1].clone()))
            }
            Self::Scalar(_) => None,
        }
    }

    pub fn is_ord(&self) -> bool {
        match self {
            Self::Scalar(ty) => ty.is_ord(),
            Self::Pair(key, value) => key.is_ord() && value.is_ord(),
        }
    }

    pub(super) fn is_float(&self) -> bool {
        match self {
            Self::Scalar(ty) => ty.contains_float(),
            Self::Pair(key, value) => key.contains_float() || value.contains_float(),
        }
    }

    pub(crate) fn contains_trace(&self) -> bool {
        match self {
            Self::Scalar(ty) => ty.contains_trace(),
            Self::Pair(key, value) => key.contains_trace() || value.contains_trace(),
        }
    }

    pub fn is_copy(&self) -> bool {
        match self {
            Self::Scalar(ty) => ty.is_copy(),
            Self::Pair(key, value) => key.is_copy() && value.is_copy(),
        }
    }
}

/// What the iterator type of the chain so far implements. `rev` needs a double ended one and
/// `rposition` one that also knows its length, `rustc` rejects the call on any other chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Caps {
    pub double_ended: bool,
    pub exact_size: bool,
    /// The chain holds no reference to a value of its own statement, so a `let` may keep it.
    pub owned: bool,
}

impl Caps {
    /// A `vec::IntoIter`, what a `Sorted` stage leaves.
    pub const VEC: Self = Self {
        double_ended: true,
        exact_size: true,
        owned: true,
    };

    /// `take`, `skip`, `step_by`, `enumerate` and `zip` run from the back only with a length.
    pub(super) fn sized_back(self) -> Self {
        Self {
            double_ended: self.double_ended && self.exact_size,
            ..self
        }
    }

    pub(super) fn unsized_len(self) -> Self {
        Self {
            exact_size: false,
            ..self
        }
    }

    pub(super) fn forward_only(self) -> Self {
        Self {
            double_ended: false,
            exact_size: false,
            ..self
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Access {
    /// `vec.into_iter()`, ordered scalar items
    VecInto,
    /// `set.into_iter()`, unordered scalar items
    SetInto,
    /// `map.into_iter()`, unordered pair items
    MapPairs,
    /// `map.into_keys()`, unordered key items
    MapKeys,
    /// `map.into_values()`, unordered value items
    MapValues,
    /// `slice.iter().cloned()` or `.copied()`, the items of a borrowed vec
    SliceIter { copied: bool },
    /// `text.chars()`
    Chars,
    /// `text.bytes()`
    Bytes,
    /// `text.lines()`, each line made a `String`
    Lines,
    /// `text.split(sep)`, each part made a `String`
    Split(char),
    /// `slice.windows(n)`, each window made a `Vec`. Panics on zero.
    Windows(u8),
    /// `slice.chunks(n)`, each chunk made a `Vec`. Panics on zero.
    Chunks(u8),
}

impl Access {
    /// Whether the source reads its collection through a reference.
    pub fn borrows(&self) -> bool {
        !matches!(
            self,
            Self::VecInto | Self::SetInto | Self::MapPairs | Self::MapKeys | Self::MapValues
        )
    }

    fn feature(&self) -> Option<&'static str> {
        match self {
            Self::VecInto | Self::SetInto | Self::MapPairs | Self::MapKeys | Self::MapValues => {
                None
            }
            Self::SliceIter { .. } => Some("lang-pipe-src-iter"),
            Self::Chars => Some("lang-pipe-src-chars"),
            Self::Bytes => Some("lang-pipe-src-bytes"),
            Self::Lines => Some("lang-pipe-src-lines"),
            Self::Split(_) => Some("lang-pipe-src-split"),
            Self::Windows(_) => Some("lang-pipe-src-windows"),
            Self::Chunks(_) => Some("lang-pipe-src-chunks"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Source {
    Coll {
        expr: Expr,
        access: Access,
    },
    /// `(start..start + count)`, ordered i64 items
    Range {
        start: i64,
        count: u8,
    },
}

impl Source {
    pub fn item(&self) -> Item {
        match self {
            Self::Range { .. } => Item::Scalar(Ty::I64),
            Self::Coll { expr, access } => {
                let ty = expr.ty();
                let elem = || ty.elem().cloned().unwrap_or(Ty::I64);
                match access {
                    Access::VecInto | Access::SetInto | Access::SliceIter { .. } => {
                        Item::Scalar(elem())
                    }
                    Access::MapPairs => match ty.key_val() {
                        Some((key, value)) => Item::Pair(key.clone(), value.clone()),
                        None => Item::Scalar(Ty::I64),
                    },
                    Access::MapKeys => match ty.key_val() {
                        Some((key, _)) => Item::Scalar(key.clone()),
                        None => Item::Scalar(Ty::I64),
                    },
                    Access::MapValues => match ty.key_val() {
                        Some((_, value)) => Item::Scalar(value.clone()),
                        None => Item::Scalar(Ty::I64),
                    },
                    Access::Chars => Item::Scalar(Ty::Char),
                    Access::Bytes => Item::Scalar(Ty::Int(IntWidth::U8)),
                    Access::Lines | Access::Split(_) => Item::Scalar(Ty::Str),
                    Access::Windows(_) | Access::Chunks(_) => Item::Scalar(Ty::vec_of(elem())),
                }
            }
        }
    }

    pub fn ordered(&self) -> bool {
        match self {
            Self::Range { .. } => true,
            Self::Coll { access, .. } => !matches!(
                access,
                Access::SetInto | Access::MapPairs | Access::MapKeys | Access::MapValues
            ),
        }
    }

    pub fn caps(&self) -> Caps {
        let (double_ended, exact_size) = match self {
            // a `Range<i64>` has no `ExactSizeIterator`
            Self::Range { .. } => (true, false),
            Self::Coll { access, .. } => match access {
                Access::VecInto
                | Access::SliceIter { .. }
                | Access::Bytes
                | Access::Windows(_)
                | Access::Chunks(_) => (true, true),
                Access::SetInto | Access::MapPairs | Access::MapKeys | Access::MapValues => {
                    (false, true)
                }
                Access::Chars | Access::Lines | Access::Split(_) => (true, false),
            },
        };
        let owned = match self {
            Self::Range { .. } => true,
            Self::Coll { access, .. } => !access.borrows(),
        };
        Caps {
            double_ended,
            exact_size,
            owned,
        }
    }

    pub(super) fn feature(&self) -> Option<&'static str> {
        match self {
            Self::Range { .. } => None,
            Self::Coll { access, .. } => access.feature(),
        }
    }

    pub(super) fn render(&self) -> String {
        match self {
            Self::Range { start, count } => {
                let end = start.saturating_add(i64::from(*count));
                format!("({start}i64..{end}i64)")
            }
            Self::Coll { expr, access } => {
                let base = expr.render();
                match access {
                    Access::VecInto | Access::SetInto | Access::MapPairs => {
                        format!("{base}.into_iter()")
                    }
                    Access::MapKeys => format!("{base}.into_keys()"),
                    Access::MapValues => format!("{base}.into_values()"),
                    Access::SliceIter { copied: true } => format!("{base}.iter().copied()"),
                    Access::SliceIter { copied: false } => format!("{base}.iter().cloned()"),
                    Access::Chars => format!("{base}.chars()"),
                    Access::Bytes => format!("{base}.bytes()"),
                    Access::Lines => {
                        format!("{base}.lines().map(|diff_part| diff_part.to_string())")
                    }
                    Access::Split(sep) => {
                        format!("{base}.split({sep:?}).map(|diff_part| diff_part.to_string())")
                    }
                    Access::Windows(size) => {
                        format!("{base}.windows({size}usize).map(|diff_part| diff_part.to_vec())")
                    }
                    Access::Chunks(size) => {
                        format!("{base}.chunks({size}usize).map(|diff_part| diff_part.to_vec())")
                    }
                }
            }
        }
    }
}
