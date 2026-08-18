//! The unboxed value model of the scalar loop plan, see `scalar_loop.rs`.
//! Every operation here mirrors one generic path exactly, `ops::arith`,
//! `ops::bit_bin`, `ops::shift_bin`, `ops::partial_compare`,
//! `ops::apply_un`, and the integer arm of `eval_cast`, running through the
//! same width-checked cores in `numeric`. A `None` answer means the generic
//! path must run this operation, which reproduces its exact panic or result.

use std::cmp::Ordering;

use num_traits::AsPrimitive;

use super::bytecode::BinKind;
use super::bytecode::UnKind;
use super::int_methods::{IntOut, int_method, takes_amount_arg};
use super::numeric::{
    IntWidth, float_arith, float_to_int, i64_arith, int_arith, int_bit, int_neg, int_not,
    int_shift, truncate, u64_arith, unify,
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
    /// An f64, mirroring `Value::Float`. An f32 stays `Opaque`, its own
    /// rounding rules live on the generic path.
    Float(f64),
    Bool(bool),
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

/// The boxed value of a slot, `None` for `Opaque`, whose frame value the
/// plan never read.
pub(super) fn s_value(v: SVal) -> Option<Value> {
    match v {
        SVal::Opaque => None,
        SVal::Unit => Some(Value::Unit),
        SVal::Int(i) => Some(Value::Int(i)),
        SVal::IntW(s, w) => Some(Value::IntW(s, w)),
        SVal::Float(f) => Some(Value::Float(f)),
        SVal::Bool(b) => Some(Value::Bool(b)),
    }
}

/// A slot as a vec index, mirroring the `usize::try_from(int_of(key))` the
/// generic `ops::index` applies. `None` sends the access to the generic
/// path, which reproduces the exact error for a negative or non-integer key.
pub(super) fn s_index(v: SVal) -> Option<usize> {
    match v {
        SVal::Int(i) => usize::try_from(i).ok(),
        SVal::IntW(s, w) => usize::try_from(w.decode(s)).ok(),
        _ => None,
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

/// Both sides as f64 when the pair mixes only floats and plain ints,
/// mirroring `ops::float_pair` for the types an `SVal` can hold. The int
/// beside a float is a bare literal the source types as f64. A width-tagged
/// int beside a float answers `None`, the generic path rejects that pair.
#[inline]
fn s_float_pair(a: SVal, b: SVal) -> Option<(f64, f64)> {
    match (a, b) {
        (SVal::Float(x), SVal::Float(y)) => Some((x, y)),
        (SVal::Int(x), SVal::Float(y)) => Some((AsPrimitive::<f64>::as_(x), y)),
        (SVal::Float(x), SVal::Int(y)) => Some((x, AsPrimitive::<f64>::as_(y))),
        _ => None,
    }
}

/// Whether a slot holds an f64, the cheap gate in front of the float pair
/// paths so all-integer loops pay one discriminant test, not a tuple match.
#[inline]
fn is_float(v: SVal) -> bool {
    matches!(v, SVal::Float(_))
}

/// `+ - * / %`, mirroring the integer and f64 paths of `ops::arith` exactly,
/// the u64 fast path included. The float gate sits behind both integer fast
/// paths on purpose, so all-integer loops pay nothing for it.
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
    // The integer order first, so all-integer loops pay nothing for the
    // float paths. A pair `s_order` cannot answer holds a float, or is not
    // comparable at all and fails over below.
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
    // `partial_cmp` carries the partial NaN semantics of
    // `ops::partial_compare` and the float arm of `Value::eq_value`: every
    // ordered comparison and `==` on a NaN is false, `!=` is true.
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
        (UnKind::Neg, SVal::Float(f)) => Some(SVal::Float(-f)),
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
        SVal::Float(f) => float_to_int(f, w),
        SVal::Bool(b) => i128::from(b),
        SVal::Opaque | SVal::Unit => return None,
    };
    from_i128(value, w)
}

/// An `as f64` cast, mirroring the `CastIr::F64` arm of `eval_cast` for the
/// types an `SVal` can hold. A bool source sends the cast to the generic
/// path, which reproduces its exact error.
pub(super) fn s_cast_f64(v: SVal) -> Option<SVal> {
    match v {
        SVal::Int(i) => Some(SVal::Float(AsPrimitive::<f64>::as_(i))),
        SVal::IntW(s, w) => Some(SVal::Float(AsPrimitive::<f64>::as_(w.decode(s)))),
        SVal::Float(f) => Some(SVal::Float(f)),
        SVal::Opaque | SVal::Unit | SVal::Bool(_) => None,
    }
}

/// `f64::from(x)`, mirroring the f64 arm of `assoc::conversion_assoc` after
/// `Value::bridge_image` flattened a width-tagged argument to a plain int,
/// its i64 saturation included. A unit argument sends the call to the
/// generic path, which reproduces its exact error.
pub(super) fn s_f64_from(v: SVal) -> Option<SVal> {
    match v {
        SVal::Float(f) => Some(SVal::Float(f)),
        SVal::Int(i) => Some(SVal::Float(AsPrimitive::<f64>::as_(i))),
        SVal::IntW(s, w) => {
            let image = i64::try_from(w.decode(s)).unwrap_or(i64::MAX);
            Some(SVal::Float(AsPrimitive::<f64>::as_(image)))
        }
        SVal::Bool(b) => Some(SVal::Float(if b { 1.0 } else { 0.0 })),
        SVal::Opaque | SVal::Unit => None,
    }
}

/// Integer methods a plan may run: pure, scalar in and out, and answered by
/// `int_method` for every integer receiver, so the plan call and the generic
/// call hit the same table. Names the table rejects at runtime, `abs` on an
/// unsigned width for one, fail the iteration over to the generic path.
pub(super) fn scalar_int_method(name: &str) -> bool {
    matches!(
        name,
        "is_multiple_of"
            | "min"
            | "max"
            | "clamp"
            | "abs"
            | "signum"
            | "pow"
            | "isqrt"
            | "div_euclid"
            | "rem_euclid"
            | "saturating_add"
            | "saturating_sub"
            | "saturating_mul"
            | "wrapping_add"
            | "wrapping_sub"
            | "wrapping_mul"
            | "wrapping_neg"
            | "count_ones"
            | "count_zeros"
            | "leading_zeros"
            | "trailing_zeros"
            | "rotate_left"
            | "rotate_right"
            | "swap_bytes"
            | "reverse_bits"
    )
}

/// Float methods a plan may run on an f64 receiver: pure, scalar in and
/// out, and answered by `s_float_method`, which mirrors the float arms of
/// `shared::num_core`.
pub(super) fn scalar_float_method(name: &str) -> bool {
    matches!(
        name,
        "sqrt"
            | "floor"
            | "ceil"
            | "round"
            | "trunc"
            | "fract"
            | "recip"
            | "powi"
            | "powf"
            | "mul_add"
            | "is_nan"
            | "is_finite"
            | "is_infinite"
            | "is_sign_positive"
            | "is_sign_negative"
    )
}

/// An integer method call, mirroring the integer arm of the generic method
/// dispatch: the same width unification `bridge::int_method` applies, then
/// the same `int_methods::int_method` table. A `None` answer, an unknown
/// name, a non-integer operand, or an error, sends the call to the generic
/// path, which reproduces its exact result or panic.
pub(super) fn s_int_method(name: &str, recv: SVal, args: &[SVal]) -> Option<SVal> {
    let (value, mut width) = parts(recv)?;
    let mut decoded = [0i128; 2];
    for (slot, arg) in decoded.iter_mut().zip(args) {
        let (arg_value, arg_width) = parts(*arg)?;
        *slot = arg_value;
        // Receiver and argument share one type in real Rust, so a width
        // either side states answers for both, except an amount argument
        // whose own u32 must not redefine the receiver.
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
        // The counting family answers u32 in real Rust, see `int_out`.
        IntOut::Count(count) => from_i128(i128::from(count), IntWidth::U32),
        IntOut::Bool(b) => Some(SVal::Bool(b)),
        _ => None,
    }
}

/// A slot as the f64 the generic `Args::float` accessor answers, after
/// `bridge_image` flattened a width-tagged int to a plain one, its i64
/// saturation included.
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

/// A float method call on an f64 receiver, mirroring the `Num::Float` arms
/// of `shared::num_core`, which nothing intercepts for a plain float: the
/// earlier dispatch steps match other receivers or other names, and a user
/// impl cannot target a primitive. A `None` answer, a non-float receiver, or
/// a bad argument sends the call to the generic path, which reproduces its
/// exact result or panic.
pub(super) fn s_float_method(name: &str, recv: SVal, args: &[SVal]) -> Option<SVal> {
    let SVal::Float(f) = recv else {
        return None;
    };
    let farg = |i: usize| args.get(i).copied().and_then(s_float_arg);
    let float = |v: f64| Some(SVal::Float(v));
    let flag = |b: bool| Some(SVal::Bool(b));
    match name {
        "sqrt" => float(f.sqrt()),
        "abs" => float(f.abs()),
        "floor" => float(f.floor()),
        "ceil" => float(f.ceil()),
        "round" => float(f.round()),
        "trunc" => float(f.trunc()),
        "fract" => float(f.fract()),
        "signum" => float(f.signum()),
        "recip" => float(f.recip()),
        "min" => float(f.min(farg(0)?)),
        "max" => float(f.max(farg(0)?)),
        "clamp" => float(f.clamp(farg(0)?, farg(1)?)),
        "powf" => float(f.powf(farg(0)?)),
        "powi" => {
            let exp = match args.first()? {
                SVal::Int(i) => *i,
                SVal::IntW(s, w) => i64::try_from(w.decode(*s)).unwrap_or(i64::MAX),
                _ => return None,
            };
            float(f.powi(i32::try_from(exp).ok()?))
        }
        "mul_add" => float(f.mul_add(farg(0)?, farg(1)?)),
        "is_nan" => flag(f.is_nan()),
        "is_finite" => flag(f.is_finite()),
        "is_infinite" => flag(f.is_infinite()),
        "is_sign_positive" => flag(f.is_sign_positive()),
        "is_sign_negative" => flag(f.is_sign_negative()),
        _ => None,
    }
}

/// The generic `is_truthy` over a slot. An `Opaque` frame value is never a
/// `Bool`, so it is falsy the same way any non-bool value is.
pub(super) fn truthy(v: SVal) -> bool {
    matches!(v, SVal::Bool(true))
}
