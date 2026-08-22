//! The unboxed value model of the scalar plans. Every operation mirrors 1 generic path exactly through
//! the same cores in `numeric`. `None` means the generic path must run this operation.

use std::cmp::Ordering;

use num_traits::AsPrimitive;

use super::bytecode::UnKind;
use super::bytecode::{BinKind, BuiltinId};
use super::int_methods::{IntOut, int_method, takes_amount_arg};
use super::numeric::{
    IntWidth, float_arith, float_to_int, i64_arith, int_arith, int_bit, int_neg, int_not,
    int_shift, truncate, u64_arith, unify,
};
use super::value::{MapKey, Value};

/// `Opaque` is a frame value the plan can't read. Reading it fails the iteration, overwriting it is
/// fine, an untouched one keeps its frame value. It is never a `Bool`, so its truthiness is false
/// like `is_truthy`.
#[derive(Clone, Copy)]
pub(super) enum SVal {
    Opaque,
    Unit,
    Int(i64),
    /// mirrors `Value::IntW`
    IntW(i64, IntWidth),
    /// an f32 stays `Opaque`, its rounding rules live on the generic path
    Float(f64),
    Bool(bool),
    /// A regex match span over the locked source. Only `MatchGet` reads one. A match past 4 GiB
    /// fails over before it becomes an item.
    Span {
        start: u32,
        end: u32,
    },
    /// A slice of the locked source, a `split_whitespace` item or the `AsStr` of a match. The map
    /// ops read one as a borrowed key.
    StrSpan {
        start: u32,
        end: u32,
    },
    /// `Ok(n)` of an `IntTryFrom`, only `UnwrapOk` reads one
    OkInt(i64),
    /// `Some(n)` of a map probe or a checked method, only `TestSome` and `UnwrapOk` read one
    SomeInt(i64),
    /// the `None` twin of `SomeInt`
    NoneOpt,
    /// a string constant, an `it["key"]` key, only the probe ops read one
    StrConst(u16),
    /// the boxed item at this index of the source of the effects runner, only `ItemIndex` reads one
    Item(u32),
    /// a boxed value in the table of the function runner, see `scalar_fn`, only the enum ops and
    /// a self call read one
    Boxed(u32),
}

impl SVal {
    pub(super) fn of(v: &Value) -> SVal {
        match v {
            Value::Unit => SVal::Unit,
            Value::Int(i) => SVal::Int(*i),
            Value::IntW(s, w) => SVal::IntW(*s, *w),
            Value::Float(f) => SVal::Float(*f),
            Value::Bool(b) => SVal::Bool(*b),
            _ => SVal::Opaque,
        }
    }
}

/// `None` for `Opaque`.
pub(super) fn s_value(v: SVal) -> Option<Value> {
    match v {
        // a span needs its source string, only `scalar_for` has it and it builds every span
        // before `write_regs` runs
        SVal::Opaque
        | SVal::Span { .. }
        | SVal::StrSpan { .. }
        | SVal::StrConst(_)
        | SVal::Item(_)
        | SVal::Boxed(_) => None,
        SVal::Unit => Some(Value::Unit),
        SVal::Int(i) => Some(Value::Int(i)),
        SVal::IntW(s, w) => Some(Value::IntW(s, w)),
        SVal::Float(f) => Some(Value::Float(f)),
        SVal::Bool(b) => Some(Value::Bool(b)),
        SVal::OkInt(n) => Some(Value::ok(Value::Int(n))),
        SVal::SomeInt(n) => Some(Value::some(Value::Int(n))),
        SVal::NoneOpt => Some(Value::none()),
    }
}

/// Mirrors `Value::as_key`. `None` sends the access to the generic path for its exact error.
pub(super) fn s_map_key(v: SVal) -> Option<MapKey> {
    match v {
        SVal::Int(i) => Some(MapKey::Int(i)),
        SVal::IntW(s, w) => Some(MapKey::Wide(s, w)),
        SVal::Bool(b) => Some(MapKey::Bool(b)),
        _ => None,
    }
}

/// Mirrors `ops::index`. `None` sends the access to the generic path for its exact error.
pub(super) fn s_index(v: SVal) -> Option<usize> {
    match v {
        SVal::Int(i) => usize::try_from(i).ok(),
        SVal::IntW(s, w) => usize::try_from(w.decode(s)).ok(),
        _ => None,
    }
}

/// Mirrors `Value::int_parts`.
fn parts(v: SVal) -> Option<(i128, IntWidth)> {
    match v {
        SVal::Int(i) => Some((i128::from(i), IntWidth::I64)),
        SVal::IntW(s, w) => Some((w.decode(s), w)),
        _ => None,
    }
}

/// Mirrors `Value::int_of_width`.
fn from_i128(v: i128, w: IntWidth) -> Option<SVal> {
    if w == IntWidth::I64 {
        i64::try_from(v).ok().map(SVal::Int)
    } else {
        Some(SVal::IntW(w.encode(v), w))
    }
}

/// Mirrors `ops::float_pair`. A width tagged int next to a float gives `None`, the generic path
/// rejects that pair.
#[inline]
fn s_float_pair(a: SVal, b: SVal) -> Option<(f64, f64)> {
    match (a, b) {
        (SVal::Float(x), SVal::Float(y)) => Some((x, y)),
        (SVal::Int(x), SVal::Float(y)) => Some((AsPrimitive::<f64>::as_(x), y)),
        (SVal::Float(x), SVal::Int(y)) => Some((x, AsPrimitive::<f64>::as_(y))),
        _ => None,
    }
}

/// The cheap gate in front of the float paths, so all integer loops pay 1 discriminant test.
#[inline]
fn is_float(v: SVal) -> bool {
    matches!(v, SVal::Float(_))
}

/// Mirrors `ops::arith`. The float gate sits behind the integer fast paths so integer loops pay
/// nothing for it.
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
    if is_float(a) || is_float(b) {
        let (lhs, rhs) = s_float_pair(a, b)?;
        return Some(SVal::Float(float_arith(op, lhs, rhs)));
    }
    let (lhs, left_width) = parts(a)?;
    let (rhs, right_width) = parts(b)?;
    let width = unify(left_width, right_width).ok()?;
    from_i128(int_arith(op, width, lhs, rhs).ok()?, width)
}

/// Mirrors the integer and bool arms of `ops::partial_compare` and `Value::eq_value`.
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
    // integer order first, so integer loops pay nothing for the float paths
    if let Some(o) = s_order(a, b) {
        return Some(match op {
            BinKind::Eq => o.is_eq(),
            BinKind::Ne => !o.is_eq(),
            BinKind::Lt => o.is_lt(),
            BinKind::Le => o.is_le(),
            BinKind::Gt => o.is_gt(),
            BinKind::Ge => o.is_ge(),
            _ => return None,
        });
    }
    // `partial_cmp` has the NaN semantics, every comparison on a NaN is false and `!=` is true
    if is_float(a) || is_float(b) {
        let (lhs, rhs) = s_float_pair(a, b)?;
        let o = lhs.partial_cmp(&rhs);
        return Some(match op {
            BinKind::Eq => o == Some(Ordering::Equal),
            BinKind::Ne => o != Some(Ordering::Equal),
            BinKind::Lt => o == Some(Ordering::Less),
            BinKind::Le => matches!(o, Some(Ordering::Less | Ordering::Equal)),
            BinKind::Gt => o == Some(Ordering::Greater),
            BinKind::Ge => matches!(o, Some(Ordering::Greater | Ordering::Equal)),
            _ => return None,
        });
    }
    None
}

/// Mirrors `ops::bit_bin`.
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

/// Mirrors `ops::shift_bin`, the result keeps the left width.
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

/// Mirrors `ops::apply_un`.
pub(super) fn s_un(op: UnKind, a: SVal) -> Option<SVal> {
    match (op, a) {
        (UnKind::Neg, SVal::Int(i)) => i.checked_neg().map(SVal::Int),
        (UnKind::Neg, SVal::IntW(s, w)) => from_i128(int_neg(w, w.decode(s)).ok()?, w),
        (UnKind::Neg, SVal::Float(f)) => Some(SVal::Float(-f)),
        (UnKind::Not, SVal::Bool(b)) => Some(SVal::Bool(!b)),
        (UnKind::Not, SVal::Int(i)) => Some(SVal::Int(!i)),
        (UnKind::Not, SVal::IntW(s, w)) => from_i128(int_not(w, w.decode(s)), w),
        _ => None,
    }
}

/// Mirrors the `CastIr::Int` arm of `eval_cast`.
pub(super) fn s_cast(v: SVal, w: IntWidth) -> Option<SVal> {
    let value = match v {
        SVal::Int(i) => truncate(i128::from(i), w),
        SVal::IntW(s, ww) => truncate(ww.decode(s), w),
        SVal::Float(f) => float_to_int(f, w),
        SVal::Bool(b) => i128::from(b),
        SVal::Opaque
        | SVal::Unit
        | SVal::Span { .. }
        | SVal::StrSpan { .. }
        | SVal::OkInt(_)
        | SVal::SomeInt(_)
        | SVal::NoneOpt
        | SVal::StrConst(_)
        | SVal::Item(_)
        | SVal::Boxed(_) => return None,
    };
    from_i128(value, w)
}

/// Mirrors the `CastIr::F64` arm of `eval_cast`. A bool source fails over.
pub(super) fn s_cast_f64(v: SVal) -> Option<SVal> {
    match v {
        SVal::Int(i) => Some(SVal::Float(AsPrimitive::<f64>::as_(i))),
        SVal::IntW(s, w) => Some(SVal::Float(AsPrimitive::<f64>::as_(w.decode(s)))),
        SVal::Float(f) => Some(SVal::Float(f)),
        SVal::Opaque
        | SVal::Unit
        | SVal::Bool(_)
        | SVal::Span { .. }
        | SVal::StrSpan { .. }
        | SVal::OkInt(_)
        | SVal::SomeInt(_)
        | SVal::NoneOpt
        | SVal::StrConst(_)
        | SVal::Item(_)
        | SVal::Boxed(_) => None,
    }
}

/// Mirrors the f64 arm of `assoc::conversion_assoc` after `bridge_image`, i64 saturation
/// included. A unit argument fails over.
pub(super) fn s_f64_from(v: SVal) -> Option<SVal> {
    match v {
        SVal::Float(f) => Some(SVal::Float(f)),
        SVal::Int(i) => Some(SVal::Float(AsPrimitive::<f64>::as_(i))),
        SVal::IntW(s, w) => {
            let image = i64::try_from(w.decode(s)).unwrap_or(i64::MAX);
            Some(SVal::Float(AsPrimitive::<f64>::as_(image)))
        }
        SVal::Bool(b) => Some(SVal::Float(if b { 1.0 } else { 0.0 })),
        SVal::Opaque
        | SVal::Unit
        | SVal::Span { .. }
        | SVal::StrSpan { .. }
        | SVal::OkInt(_)
        | SVal::SomeInt(_)
        | SVal::NoneOpt
        | SVal::StrConst(_)
        | SVal::Item(_)
        | SVal::Boxed(_) => None,
    }
}

/// 1 variant per distinct arm of `assoc::int_fits`.
#[derive(Clone, Copy)]
pub(super) enum TryFits {
    I8,
    I16,
    I32,
    U8,
    U16,
    U32,
    NonNeg,
    Any,
}

/// Mirrors `assoc::int_fits`.
pub(super) fn try_fits_of(ty: &str) -> Option<TryFits> {
    Some(match ty {
        "i8" => TryFits::I8,
        "i16" => TryFits::I16,
        "i32" => TryFits::I32,
        "u8" => TryFits::U8,
        "u16" => TryFits::U16,
        "u32" => TryFits::U32,
        "u64" | "u128" | "usize" => TryFits::NonNeg,
        "i64" | "i128" | "isize" => TryFits::Any,
        _ => return None,
    })
}

/// `as_str`, `to_string` or `to_owned` on a span slot stays a span, the owned copy is deferred to the
/// site that needs one. Any other receiver fails over.
pub(super) fn s_as_str(v: SVal) -> Option<SVal> {
    match v {
        SVal::Span { start, end } | SVal::StrSpan { start, end } => {
            Some(SVal::StrSpan { start, end })
        }
        _ => None,
    }
}

/// Mirrors the `MatchOut::Int` arms of `shared::match_core`. Any other receiver fails over.
pub(super) fn s_match_get(v: SVal, end: bool) -> Option<SVal> {
    match v {
        SVal::Span { start, end: stop } => {
            Some(SVal::Int(i64::from(if end { stop } else { start })))
        }
        _ => None,
    }
}

/// `.unwrap()` on an `OkInt` or `SomeInt` slot. Any other receiver fails over, the `Err` or
/// `None` panic included.
pub(super) fn s_unwrap_ok(v: SVal) -> Option<SVal> {
    match v {
        SVal::OkInt(n) | SVal::SomeInt(n) => Some(SVal::Int(n)),
        _ => None,
    }
}

/// Mirrors the `try_from` arm of `assoc::conversion_assoc`. `None` sends the call to the generic
/// path, which builds the real `Err`.
pub(super) fn s_try_from(fits: TryFits, v: SVal) -> Option<SVal> {
    let n = match v {
        SVal::Int(n) => n,
        SVal::Bool(b) => i64::from(b),
        SVal::IntW(s, w) => i64::try_from(w.decode(s)).ok()?,
        _ => return None,
    };
    let ok = match fits {
        TryFits::I8 => i8::try_from(n).is_ok(),
        TryFits::I16 => i16::try_from(n).is_ok(),
        TryFits::I32 => i32::try_from(n).is_ok(),
        TryFits::U8 => u8::try_from(n).is_ok(),
        TryFits::U16 => u16::try_from(n).is_ok(),
        TryFits::U32 => u32::try_from(n).is_ok(),
        TryFits::NonNeg => n >= 0,
        TryFits::Any => true,
    };
    ok.then_some(SVal::OkInt(n))
}

/// Pure, scalar in and out, and goes through `int_method` so the plan and the generic call hit the
/// same table. A name the table rejects fails over.
pub(super) fn scalar_int_method(name: BuiltinId) -> bool {
    matches!(
        name,
        BuiltinId::IsMultipleOf
            | BuiltinId::Min
            | BuiltinId::Max
            | BuiltinId::Clamp
            | BuiltinId::Abs
            | BuiltinId::Signum
            | BuiltinId::Pow
            | BuiltinId::Isqrt
            | BuiltinId::DivEuclid
            | BuiltinId::RemEuclid
            | BuiltinId::SaturatingAdd
            | BuiltinId::SaturatingSub
            | BuiltinId::SaturatingMul
            | BuiltinId::WrappingAdd
            | BuiltinId::WrappingSub
            | BuiltinId::WrappingMul
            | BuiltinId::WrappingNeg
            | BuiltinId::CountOnes
            | BuiltinId::CountZeros
            | BuiltinId::LeadingZeros
            | BuiltinId::TrailingZeros
            | BuiltinId::RotateLeft
            | BuiltinId::RotateRight
            | BuiltinId::SwapBytes
            | BuiltinId::ReverseBits
            | BuiltinId::AsI64
            | BuiltinId::AsU64
    )
}

/// Pure, scalar in and out, goes through `s_float_method`.
pub(super) fn scalar_float_method(name: BuiltinId) -> bool {
    matches!(
        name,
        BuiltinId::Sqrt
            | BuiltinId::Floor
            | BuiltinId::Ceil
            | BuiltinId::Round
            | BuiltinId::Trunc
            | BuiltinId::Fract
            | BuiltinId::Recip
            | BuiltinId::Powi
            | BuiltinId::Powf
            | BuiltinId::MulAdd
            | BuiltinId::IsNan
            | BuiltinId::IsFinite
            | BuiltinId::IsInfinite
            | BuiltinId::IsSignPositive
            | BuiltinId::IsSignNegative
    )
}

/// Mirrors `bridge::int_method` and the `int_methods::int_method` table. `None` sends the call to
/// the generic path.
pub(super) fn s_int_method(name: BuiltinId, recv: SVal, args: &[SVal]) -> Option<SVal> {
    let (value, mut width) = parts(recv)?;
    let mut decoded = [0i128; 2];
    for (slot, arg) in decoded.iter_mut().zip(args) {
        let (arg_value, arg_width) = parts(*arg)?;
        *slot = arg_value;
        // either width works for both, except a shift amount's u32
        if !takes_amount_arg(name)
            && let Ok(unified) = unify(width, arg_width)
        {
            width = unified;
        }
    }
    if width.is_big() {
        return None;
    }
    match int_method(name, width, value, &decoded[..args.len()])?.ok()? {
        IntOut::Same(v) => from_i128(v, width),
        // counts are u32, see `int_out`
        IntOut::Count(count) => from_i128(i128::from(count), IntWidth::U32),
        IntOut::Bool(b) => Some(SVal::Bool(b)),
        // only the plain int width has the `SomeInt` slot form
        IntOut::Checked(opt) if width == IntWidth::I64 => Some(match opt {
            Some(v) => SVal::SomeInt(i64::try_from(v).ok()?),
            None => SVal::NoneOpt,
        }),
        _ => None,
    }
}

/// Mirrors `Args::float` after `bridge_image`, i64 saturation included.
fn s_float_arg(v: SVal) -> Option<f64> {
    match v {
        SVal::Float(f) => Some(f),
        SVal::Int(i) => Some(AsPrimitive::<f64>::as_(i)),
        SVal::IntW(s, w) => {
            let image = i64::try_from(w.decode(s)).unwrap_or(i64::MAX);
            Some(AsPrimitive::<f64>::as_(image))
        }
        _ => None,
    }
}

/// Mirrors the `Num::Float` arms of `shared::num_core`, nothing intercepts those for a plain
/// float. `None` sends the call to the generic path.
pub(super) fn s_float_method(name: BuiltinId, recv: SVal, args: &[SVal]) -> Option<SVal> {
    let SVal::Float(f) = recv else {
        return None;
    };
    let farg = |i: usize| args.get(i).copied().and_then(s_float_arg);
    let float = |v: f64| Some(SVal::Float(v));
    let flag = |b: bool| Some(SVal::Bool(b));
    match name {
        BuiltinId::Sqrt => float(f.sqrt()),
        BuiltinId::Abs => float(f.abs()),
        BuiltinId::Floor => float(f.floor()),
        BuiltinId::Ceil => float(f.ceil()),
        BuiltinId::Round => float(f.round()),
        BuiltinId::Trunc => float(f.trunc()),
        BuiltinId::Fract => float(f.fract()),
        BuiltinId::Signum => float(f.signum()),
        BuiltinId::Recip => float(f.recip()),
        BuiltinId::Min => float(f.min(farg(0)?)),
        BuiltinId::Max => float(f.max(farg(0)?)),
        BuiltinId::Clamp => float(f.clamp(farg(0)?, farg(1)?)),
        BuiltinId::Powf => float(f.powf(farg(0)?)),
        BuiltinId::Powi => {
            let exp = match args.first()? {
                SVal::Int(i) => *i,
                SVal::IntW(s, w) => i64::try_from(w.decode(*s)).unwrap_or(i64::MAX),
                _ => return None,
            };
            float(f.powi(i32::try_from(exp).ok()?))
        }
        BuiltinId::MulAdd => float(f.mul_add(farg(0)?, farg(1)?)),
        BuiltinId::IsNan => flag(f.is_nan()),
        BuiltinId::IsFinite => flag(f.is_finite()),
        BuiltinId::IsInfinite => flag(f.is_infinite()),
        BuiltinId::IsSignPositive => flag(f.is_sign_positive()),
        BuiltinId::IsSignNegative => flag(f.is_sign_negative()),
        _ => None,
    }
}

/// Mirrors `is_truthy`, an `Opaque` is never a `Bool` so it is falsy.
pub(super) fn truthy(v: SVal) -> bool {
    matches!(v, SVal::Bool(true))
}
