//! The unboxed value model of the scalar loop plan, see `scalar_loop.rs`.
//! Every operation here mirrors one generic path exactly, `ops::arith`,
//! `ops::bit_bin`, `ops::shift_bin`, `ops::partial_compare`,
//! `ops::apply_un`, and the integer arm of `eval_cast`, running through the
//! same width-checked cores in `numeric`. A `None` answer means the generic
//! path must run this operation, which reproduces its exact panic or result.

use std::cmp::Ordering;

use super::bytecode::BinKind;
use super::bytecode::UnKind;
use super::numeric::{
    IntWidth, i64_arith, int_arith, int_bit, int_neg, int_not, int_shift, truncate, u64_arith,
    unify,
};
use super::value::Value;

/// An unboxed register value. `Opaque` stands for a frame value the plan
/// cannot read: reading it fails the iteration, overwriting it is fine, and
/// an untouched one keeps its frame value through writeback. An `Opaque`
/// frame value is never a `Bool`, those load as `Bool`, so its truthiness
/// is a constant false exactly like the generic `is_truthy`.
#[derive(Clone, Copy)]
pub(super) enum SVal {
    Opaque,
    Unit,
    Int(i64),
    /// Storage form plus width, mirroring `Value::IntW`.
    IntW(i64, IntWidth),
    Bool(bool),
}

impl SVal {
    pub(super) fn of(v: &Value) -> SVal {
        match v {
            Value::Unit => SVal::Unit,
            Value::Int(i) => SVal::Int(*i),
            Value::IntW(s, w) => SVal::IntW(*s, *w),
            Value::Bool(b) => SVal::Bool(*b),
            _ => SVal::Opaque,
        }
    }
}

/// The decoded value and width of an integer slot, mirroring
/// `Value::int_parts` for the widths an `SVal` can hold.
fn parts(v: SVal) -> Option<(i128, IntWidth)> {
    match v {
        SVal::Int(i) => Some((i128::from(i), IntWidth::I64)),
        SVal::IntW(s, w) => Some((w.decode(s), w)),
        _ => None,
    }
}

/// Build an integer of the given width, mirroring `Value::int_of_width` for
/// the one-i64 widths.
fn from_i128(v: i128, w: IntWidth) -> Option<SVal> {
    if w == IntWidth::I64 {
        i64::try_from(v).ok().map(SVal::Int)
    } else {
        Some(SVal::IntW(w.encode(v), w))
    }
}

/// `+ - * / %`, mirroring the integer paths of `ops::arith` exactly,
/// the u64 fast path included.
#[inline]
fn s_arith(op: BinKind, a: SVal, b: SVal) -> Option<SVal> {
    if let (SVal::Int(lhs), SVal::Int(rhs)) = (a, b) {
        return i64_arith(op, lhs, rhs).ok().map(SVal::Int);
    }
    if let SVal::IntW(lhs, width @ (IntWidth::U64 | IntWidth::USize)) = a {
        let rhs = match b {
            SVal::IntW(rhs, right_width) if right_width == width => Some(rhs.cast_unsigned()),
            SVal::Int(rhs) if rhs >= 0 => Some(rhs.cast_unsigned()),
            _ => None,
        };
        if let Some(rhs) = rhs {
            let out = u64_arith(op, lhs.cast_unsigned(), rhs).ok()?;
            return Some(SVal::IntW(out.cast_signed(), width));
        }
    }
    let (lhs, left_width) = parts(a)?;
    let (rhs, right_width) = parts(b)?;
    let width = unify(left_width, right_width).ok()?;
    from_i128(int_arith(op, width, lhs, rhs).ok()?, width)
}

/// The order of two slots, mirroring the integer and bool arms of
/// `ops::partial_compare` and `Value::eq_value`, which agree on these types.
fn s_order(a: SVal, b: SVal) -> Option<Ordering> {
    match (a, b) {
        (SVal::Int(lhs), SVal::Int(rhs)) => Some(lhs.cmp(&rhs)),
        (SVal::Bool(lhs), SVal::Bool(rhs)) => Some(lhs.cmp(&rhs)),
        _ => {
            let (lhs, _) = parts(a)?;
            let (rhs, _) = parts(b)?;
            Some(lhs.cmp(&rhs))
        }
    }
}

#[inline]
pub(super) fn s_cmp(op: BinKind, a: SVal, b: SVal) -> Option<bool> {
    let o = s_order(a, b)?;
    Some(match op {
        BinKind::Eq => o.is_eq(),
        BinKind::Ne => !o.is_eq(),
        BinKind::Lt => o.is_lt(),
        BinKind::Le => o.is_le(),
        BinKind::Gt => o.is_gt(),
        BinKind::Ge => o.is_ge(),
        _ => return None,
    })
}

/// `& | ^`, mirroring `ops::bit_bin`.
fn s_bit(op: BinKind, a: SVal, b: SVal) -> Option<SVal> {
    let bits = |lhs: i64, rhs: i64| match op {
        BinKind::BitAnd => lhs & rhs,
        BinKind::BitOr => lhs | rhs,
        _ => lhs ^ rhs,
    };
    match (a, b) {
        (SVal::Int(lhs), SVal::Int(rhs)) => Some(SVal::Int(bits(lhs, rhs))),
        (SVal::Bool(lhs), SVal::Bool(rhs)) => {
            Some(SVal::Bool(bits(i64::from(lhs), i64::from(rhs)) != 0))
        }
        _ => {
            let (lhs, left_width) = parts(a)?;
            let (rhs, right_width) = parts(b)?;
            let width = unify(left_width, right_width).ok()?;
            from_i128(int_bit(op, lhs, rhs).ok()?, width)
        }
    }
}

/// `<< >>`, mirroring `ops::shift_bin`: the result keeps the left width.
fn s_shift(op: BinKind, a: SVal, b: SVal) -> Option<SVal> {
    let (lhs, width) = parts(a)?;
    let (rhs, _) = parts(b)?;
    from_i128(int_shift(op, width, lhs, rhs).ok()?, width)
}

#[inline]
pub(super) fn s_bin(op: BinKind, a: SVal, b: SVal) -> Option<SVal> {
    use BinKind::{
        Add, BitAnd, BitOr, BitXor, Div, Eq, Ge, Gt, Le, Lt, Mul, Ne, Rem, Shl, Shr, Sub,
    };
    match op {
        Add | Sub | Mul | Div | Rem => s_arith(op, a, b),
        Eq | Ne | Lt | Le | Gt | Ge => s_cmp(op, a, b).map(SVal::Bool),
        BitAnd | BitOr | BitXor => s_bit(op, a, b),
        Shl | Shr => s_shift(op, a, b),
    }
}

/// `- !`, mirroring `ops::apply_un`.
pub(super) fn s_un(op: UnKind, a: SVal) -> Option<SVal> {
    match (op, a) {
        (UnKind::Neg, SVal::Int(i)) => i.checked_neg().map(SVal::Int),
        (UnKind::Neg, SVal::IntW(s, w)) => from_i128(int_neg(w, w.decode(s)).ok()?, w),
        (UnKind::Not, SVal::Bool(b)) => Some(SVal::Bool(!b)),
        (UnKind::Not, SVal::Int(i)) => Some(SVal::Int(!i)),
        (UnKind::Not, SVal::IntW(s, w)) => from_i128(int_not(w, w.decode(s)), w),
        _ => None,
    }
}

/// An `as` cast to an integer width, mirroring the `CastIr::Int` arm of
/// `eval_cast`.
pub(super) fn s_cast(v: SVal, w: IntWidth) -> Option<SVal> {
    let value = match v {
        SVal::Int(i) => truncate(i128::from(i), w),
        SVal::IntW(s, ww) => truncate(ww.decode(s), w),
        SVal::Bool(b) => i128::from(b),
        SVal::Opaque | SVal::Unit => return None,
    };
    from_i128(value, w)
}

/// The generic `is_truthy` over a slot. An `Opaque` frame value is never a
/// `Bool`, so it is falsy the same way any non-bool value is.
pub(super) fn truthy(v: SVal) -> bool {
    matches!(v, SVal::Bool(true))
}
