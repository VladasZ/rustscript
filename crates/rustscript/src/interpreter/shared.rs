//! Shared method cores for scalar receivers. A core works on plain Rust types, so the dispatch
//! layer only adapts values and the coverage harvest reads each core once. Nothing lazy or
//! stateful here.

use num_traits::{AsPrimitive, PrimInt};
use std::cmp::Ordering;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};

use super::bytecode::{BinKind, BuiltinId, ScalarTy};
use super::numeric::IntWidth;

/// The cores monomorphize over this, the view is free.
pub(super) trait Args {
    /// what `Display` would print, missing arguments are empty
    fn text(&self, i: usize) -> String;
    fn int(&self, i: usize) -> Option<i64>;
    fn float(&self, i: usize) -> Option<f64>;
    /// the chars of a `['-', '_']` pattern, a char set splits on any member
    fn pattern_chars(&self, i: usize) -> Option<Vec<char>>;
}

fn int_arg(args: &impl Args, i: usize) -> Result<i64> {
    match args.int(i) {
        Some(n) => Ok(n),
        None => bail!("expected an integer argument"),
    }
}

/// A negative or oversized value can only be an interpreter bug, so error instead of wrapping.
fn usize_arg(args: &impl Args, i: usize) -> Result<usize> {
    let n = int_arg(args, i)?;
    usize::try_from(n).map_err(|_| anyhow!("`{n}` is not a valid count"))
}

/// Lengths fit in i64 on every platform we support.
pub(super) fn usize_i64(i: usize) -> i64 {
    i64::try_from(i).expect("value exceeds i64")
}

/// A length with its `usize` tag. Without the tag `!v.len()` is a small negative number instead
/// of a huge unsigned one.
pub(super) fn usize_value(i: usize) -> super::value::Value {
    super::value::Value::int_of_width(i128::from(usize_i64(i)), IntWidth::USize)
}

fn float_arg(args: &impl Args, i: usize) -> Result<f64> {
    match args.float(i) {
        Some(f) => Ok(f),
        None => bail!("expected a float argument"),
    }
}

// numbers

#[derive(Clone, Copy)]
pub(super) enum Num {
    Int(i64),
    Float(f64),
}

pub(super) enum NumOut {
    Int(i64),
    Float(f64),
    Bool(bool),
    Bytes(Vec<u8>),
    SomeInt(i64),
    SomeFloat(f64),
    Nothing,
    Ordering(Ordering),
    SomeOrdering(Ordering),
}

pub(super) fn num_core(recv: Num, name: BuiltinId, args: &impl Args) -> Result<Option<NumOut>> {
    use Num::{Float, Int};
    let as_f = || match recv {
        Int(i) => AsPrimitive::<f64>::as_(i),
        Float(f) => f,
    };
    Ok(Some(match (recv, name) {
        // `as_i64`, `as_u64` and `as_f64` on an integer are handled in `int_methods`
        (Int(i), BuiltinId::AsI128 | BuiltinId::AsUsize) => NumOut::SomeInt(i),
        (Float(f), BuiltinId::AsF64) => NumOut::SomeFloat(f),
        // serde_json integer accessors return None on a float, even on 5.0
        (
            Float(_),
            BuiltinId::AsI64 | BuiltinId::AsU64 | BuiltinId::AsI128 | BuiltinId::AsUsize,
        )
        | (
            _,
            BuiltinId::AsStr
            | BuiltinId::AsBool
            | BuiltinId::AsArray
            | BuiltinId::AsArrayMut
            | BuiltinId::AsObject
            | BuiltinId::AsObjectMut,
        ) => NumOut::Nothing,
        (Int(i), BuiltinId::Abs) => NumOut::Int(i.abs()),
        (Float(f), BuiltinId::Abs) => NumOut::Float(f.abs()),
        (Int(i), BuiltinId::Pow) => NumOut::Int(i.pow(u32::try_from(int_arg(args, 0)?)?)),
        (Float(f), BuiltinId::Powi) => NumOut::Float(f.powi(i32::try_from(int_arg(args, 0)?)?)),
        (Float(f), BuiltinId::Powf) => NumOut::Float(f.powf(float_arg(args, 0)?)),
        (Float(f), BuiltinId::Sqrt) => NumOut::Float(f.sqrt()),
        (Float(f), BuiltinId::Cbrt) => NumOut::Float(f.cbrt()),
        (Float(f), BuiltinId::Exp) => NumOut::Float(f.exp()),
        (Float(f), BuiltinId::Exp2) => NumOut::Float(f.exp2()),
        (Float(f), BuiltinId::Ln) => NumOut::Float(f.ln()),
        (Float(f), BuiltinId::Log2) => NumOut::Float(f.log2()),
        (Float(f), BuiltinId::Log10) => NumOut::Float(f.log10()),
        (Float(f), BuiltinId::ToDegrees) => NumOut::Float(f.to_degrees()),
        (Float(f), BuiltinId::ToRadians) => NumOut::Float(f.to_radians()),
        (Float(f), BuiltinId::RoundTiesEven) => NumOut::Float(f.round_ties_even()),
        (Float(f), BuiltinId::Hypot) => NumOut::Float(f.hypot(float_arg(args, 0)?)),
        (Float(f), BuiltinId::Copysign) => NumOut::Float(f.copysign(float_arg(args, 0)?)),
        (Float(f), BuiltinId::Midpoint) => NumOut::Float(f.midpoint(float_arg(args, 0)?)),
        (Float(f), BuiltinId::RemEuclid) => NumOut::Float(f.rem_euclid(float_arg(args, 0)?)),
        (Float(f), BuiltinId::DivEuclid) => NumOut::Float(f.div_euclid(float_arg(args, 0)?)),
        (Float(f), BuiltinId::IsNormal) => NumOut::Bool(f.is_normal()),
        (Float(f), BuiltinId::IsSubnormal) => NumOut::Bool(f.is_subnormal()),
        (Float(f), BuiltinId::Floor) => NumOut::Float(f.floor()),
        (Float(f), BuiltinId::Trunc) => NumOut::Float(f.trunc()),
        // The untyped `parse` guesses "160" into an int, so float methods on an int go through
        // the float view.
        (Int(i), BuiltinId::Trunc | BuiltinId::Floor | BuiltinId::Ceil | BuiltinId::Round) => {
            NumOut::Int(i)
        }
        (Int(_), BuiltinId::Sqrt) => NumOut::Float(as_f().sqrt()),
        (Int(_), BuiltinId::Powi) => NumOut::Float(as_f().powi(i32::try_from(int_arg(args, 0)?)?)),
        (Int(_), BuiltinId::Powf) => NumOut::Float(as_f().powf(float_arg(args, 0)?)),
        (Int(i), BuiltinId::IsSignPositive) => NumOut::Bool(i >= 0),
        (Float(f), BuiltinId::Ceil) => NumOut::Float(f.ceil()),
        (Float(f), BuiltinId::Round) => NumOut::Float(f.round()),
        (Float(f), BuiltinId::IsSignPositive) => NumOut::Bool(f.is_sign_positive()),
        (Float(f), BuiltinId::Fract) => NumOut::Float(f.fract()),
        (Int(_), BuiltinId::Fract) => NumOut::Int(0),
        // int `signum` lives in `int_methods`
        (Float(f), BuiltinId::Signum) => NumOut::Float(f.signum()),
        (Float(f), BuiltinId::Recip) => NumOut::Float(f.recip()),
        (Int(_), BuiltinId::Recip) => NumOut::Float(as_f().recip()),
        (Float(f), BuiltinId::MulAdd) => {
            NumOut::Float(f.mul_add(float_arg(args, 0)?, float_arg(args, 1)?))
        }
        (Int(_), BuiltinId::MulAdd) => {
            NumOut::Float(as_f().mul_add(float_arg(args, 0)?, float_arg(args, 1)?))
        }
        (Float(f), BuiltinId::IsNan) => NumOut::Bool(f.is_nan()),
        (Float(f), BuiltinId::IsFinite) => NumOut::Bool(f.is_finite()),
        (Int(_), BuiltinId::IsFinite) => NumOut::Bool(true),
        (Float(f), BuiltinId::IsInfinite) => NumOut::Bool(f.is_infinite()),
        (Int(_), BuiltinId::IsNan | BuiltinId::IsInfinite) => NumOut::Bool(false),
        (Float(f), BuiltinId::IsSignNegative) => NumOut::Bool(f.is_sign_negative()),
        (Int(i), BuiltinId::IsSignNegative) => NumOut::Bool(i < 0),
        (Int(a), BuiltinId::Min) => NumOut::Int(a.min(int_arg(args, 0)?)),
        (Int(a), BuiltinId::Max) => NumOut::Int(a.max(int_arg(args, 0)?)),
        (Int(a), BuiltinId::Clamp) => NumOut::Int(a.clamp(int_arg(args, 0)?, int_arg(args, 1)?)),
        (Float(a), BuiltinId::Clamp) => {
            let (low, high) = (float_arg(args, 0)?, float_arg(args, 1)?);
            if !matches!(
                low.partial_cmp(&high),
                Some(Ordering::Less | Ordering::Equal)
            ) {
                bail!("min > max, or either was NaN. min = {low:?}, max = {high:?}");
            }
            NumOut::Float(a.clamp(low, high))
        }
        (Float(a), BuiltinId::Min) => NumOut::Float(a.min(float_arg(args, 0)?)),
        (Float(a), BuiltinId::Max) => NumOut::Float(a.max(float_arg(args, 0)?)),
        (Int(a), BuiltinId::IsMultipleOf) => NumOut::Bool(a % int_arg(args, 0)? == 0),
        (Int(a), BuiltinId::SaturatingSub) => NumOut::Int(a.saturating_sub(int_arg(args, 0)?)),
        (Int(a), BuiltinId::SaturatingAdd) => NumOut::Int(a.saturating_add(int_arg(args, 0)?)),
        (Int(a), BuiltinId::SaturatingMul) => NumOut::Int(a.saturating_mul(int_arg(args, 0)?)),
        (Int(a), BuiltinId::Cmp) => NumOut::Ordering(a.cmp(&int_arg(args, 0)?)),
        (_, BuiltinId::PartialCmp) => NumOut::SomeOrdering(
            as_f()
                .partial_cmp(&float_arg(args, 0)?)
                .unwrap_or(Ordering::Equal),
        ),
        _ => return float_extra(recv, name, args),
    }))
}

/// Trig, total ordering and byte conversions on f64. Split out of `num_core` so that
/// function stays inside the line limit.
fn float_extra(recv: Num, name: BuiltinId, args: &impl Args) -> Result<Option<NumOut>> {
    let Num::Float(f) = recv else {
        return Ok(None);
    };
    Ok(Some(match name {
        BuiltinId::Sin => NumOut::Float(f.sin()),
        BuiltinId::Cos => NumOut::Float(f.cos()),
        BuiltinId::Tan => NumOut::Float(f.tan()),
        BuiltinId::Asin => NumOut::Float(f.asin()),
        BuiltinId::Acos => NumOut::Float(f.acos()),
        BuiltinId::Atan => NumOut::Float(f.atan()),
        BuiltinId::Atan2 => NumOut::Float(f.atan2(float_arg(args, 0)?)),
        BuiltinId::Sinh => NumOut::Float(f.sinh()),
        BuiltinId::Cosh => NumOut::Float(f.cosh()),
        BuiltinId::Tanh => NumOut::Float(f.tanh()),
        BuiltinId::TotalCmp => NumOut::Ordering(f.total_cmp(&float_arg(args, 0)?)),
        BuiltinId::ToLeBytes => NumOut::Bytes(f.to_le_bytes().to_vec()),
        BuiltinId::ToBeBytes => NumOut::Bytes(f.to_be_bytes().to_vec()),
        BuiltinId::ToNeBytes => NumOut::Bytes(f.to_ne_bytes().to_vec()),
        _ => return Ok(None),
    }))
}

pub(super) enum F32Out {
    Val(f32),
    Bool(bool),
    Bytes(Vec<u8>),
    Ordering(Ordering),
    SomeOrdering(Ordering),
}

/// Computed in real f32. Through the f64 core `sqrt` double rounds and `{:?}` prints
/// `3.4028234663852886e38` instead of `3.4028235e38`.
pub(super) fn f32_core(recv: f32, name: BuiltinId, args: &impl Args) -> Result<Option<F32Out>> {
    let arg = |i: usize| -> Result<f32> { float_arg(args, i).map(AsPrimitive::<f32>::as_) };
    Ok(Some(match name {
        BuiltinId::Abs => F32Out::Val(recv.abs()),
        BuiltinId::Powi => F32Out::Val(recv.powi(i32::try_from(int_arg(args, 0)?)?)),
        BuiltinId::Powf => F32Out::Val(recv.powf(arg(0)?)),
        BuiltinId::Sqrt => F32Out::Val(recv.sqrt()),
        BuiltinId::Cbrt => F32Out::Val(recv.cbrt()),
        BuiltinId::Exp => F32Out::Val(recv.exp()),
        BuiltinId::Exp2 => F32Out::Val(recv.exp2()),
        BuiltinId::Ln => F32Out::Val(recv.ln()),
        BuiltinId::Log2 => F32Out::Val(recv.log2()),
        BuiltinId::Log10 => F32Out::Val(recv.log10()),
        BuiltinId::ToDegrees => F32Out::Val(recv.to_degrees()),
        BuiltinId::ToRadians => F32Out::Val(recv.to_radians()),
        BuiltinId::RoundTiesEven => F32Out::Val(recv.round_ties_even()),
        BuiltinId::Hypot => F32Out::Val(recv.hypot(arg(0)?)),
        BuiltinId::Sin => F32Out::Val(recv.sin()),
        BuiltinId::Cos => F32Out::Val(recv.cos()),
        BuiltinId::Tan => F32Out::Val(recv.tan()),
        BuiltinId::Asin => F32Out::Val(recv.asin()),
        BuiltinId::Acos => F32Out::Val(recv.acos()),
        BuiltinId::Atan => F32Out::Val(recv.atan()),
        BuiltinId::Atan2 => F32Out::Val(recv.atan2(arg(0)?)),
        BuiltinId::Sinh => F32Out::Val(recv.sinh()),
        BuiltinId::Cosh => F32Out::Val(recv.cosh()),
        BuiltinId::Tanh => F32Out::Val(recv.tanh()),
        BuiltinId::TotalCmp => F32Out::Ordering(recv.total_cmp(&arg(0)?)),
        BuiltinId::ToLeBytes => F32Out::Bytes(recv.to_le_bytes().to_vec()),
        BuiltinId::ToBeBytes => F32Out::Bytes(recv.to_be_bytes().to_vec()),
        BuiltinId::ToNeBytes => F32Out::Bytes(recv.to_ne_bytes().to_vec()),
        BuiltinId::Copysign => F32Out::Val(recv.copysign(arg(0)?)),
        BuiltinId::Midpoint => F32Out::Val(recv.midpoint(arg(0)?)),
        BuiltinId::RemEuclid => F32Out::Val(recv.rem_euclid(arg(0)?)),
        BuiltinId::DivEuclid => F32Out::Val(recv.div_euclid(arg(0)?)),
        BuiltinId::IsNormal => F32Out::Bool(recv.is_normal()),
        BuiltinId::IsSubnormal => F32Out::Bool(recv.is_subnormal()),
        BuiltinId::Floor => F32Out::Val(recv.floor()),
        BuiltinId::Trunc => F32Out::Val(recv.trunc()),
        BuiltinId::Ceil => F32Out::Val(recv.ceil()),
        BuiltinId::Round => F32Out::Val(recv.round()),
        BuiltinId::Min => F32Out::Val(recv.min(arg(0)?)),
        BuiltinId::Max => F32Out::Val(recv.max(arg(0)?)),
        BuiltinId::Clamp => {
            let (low, high) = (arg(0)?, arg(1)?);
            if !matches!(
                low.partial_cmp(&high),
                Some(Ordering::Less | Ordering::Equal)
            ) {
                bail!("min > max, or either was NaN. min = {low:?}, max = {high:?}");
            }
            F32Out::Val(recv.clamp(low, high))
        }
        BuiltinId::Fract => F32Out::Val(recv.fract()),
        BuiltinId::Signum => F32Out::Val(recv.signum()),
        BuiltinId::Recip => F32Out::Val(recv.recip()),
        BuiltinId::MulAdd => F32Out::Val(recv.mul_add(arg(0)?, arg(1)?)),
        BuiltinId::IsSignPositive => F32Out::Bool(recv.is_sign_positive()),
        BuiltinId::IsSignNegative => F32Out::Bool(recv.is_sign_negative()),
        BuiltinId::IsNan => F32Out::Bool(recv.is_nan()),
        BuiltinId::IsFinite => F32Out::Bool(recv.is_finite()),
        BuiltinId::IsInfinite => F32Out::Bool(recv.is_infinite()),
        // same as the f64 core
        BuiltinId::PartialCmp => {
            F32Out::SomeOrdering(recv.partial_cmp(&arg(0)?).unwrap_or(Ordering::Equal))
        }
        _ => return Ok(None),
    }))
}

// json

/// Parsed json is held as plain values, so the serde type tests are shape tests.
#[derive(Clone, Copy)]
pub(super) enum JsonKind {
    Object,
    Array,
    Str,
    Bool,
    /// real value, so the range tests work
    Int(i128),
    Float,
    Null,
    Other,
}

/// Runs before the per type dispatch, which returns early for the hot receivers.
pub(super) fn json_type_test(kind: JsonKind, name: BuiltinId) -> Option<bool> {
    Some(match name {
        BuiltinId::IsObject => matches!(kind, JsonKind::Object),
        BuiltinId::IsArray => matches!(kind, JsonKind::Array),
        BuiltinId::IsString => matches!(kind, JsonKind::Str),
        BuiltinId::IsBoolean => matches!(kind, JsonKind::Bool),
        BuiltinId::IsNumber => matches!(kind, JsonKind::Int(_) | JsonKind::Float),
        // serde checks by range, a negative number is not a u64
        BuiltinId::IsI64 => matches!(kind, JsonKind::Int(v) if i64::try_from(v).is_ok()),
        BuiltinId::IsU64 => matches!(kind, JsonKind::Int(v) if u64::try_from(v).is_ok()),
        BuiltinId::IsF64 => matches!(kind, JsonKind::Float),
        BuiltinId::IsNull => matches!(kind, JsonKind::Null),
        _ => return None,
    })
}

/// By name only, the caller decides if the receiver matches. A wrong shape gives None.
pub(super) fn json_accessor(name: BuiltinId) -> bool {
    matches!(
        name,
        BuiltinId::AsStr
            | BuiltinId::AsI64
            | BuiltinId::AsU64
            | BuiltinId::AsF64
            | BuiltinId::AsBool
            | BuiltinId::AsArray
            | BuiltinId::AsArrayMut
            | BuiltinId::AsObject
            | BuiltinId::AsObjectMut
    )
}

/// RFC 6901. An empty pointer is the whole value. `~1` and `~0` escape slash and tilde.
pub(super) fn json_pointer_tokens(pointer: &str) -> Option<Vec<String>> {
    if pointer.is_empty() {
        return Some(Vec::new());
    }
    if !pointer.starts_with('/') {
        return None;
    }
    Some(
        pointer
            .split('/')
            .skip(1)
            .map(|token| token.replace("~1", "/").replace("~0", "~"))
            .collect(),
    )
}

/// serde rejects a leading plus and a leading zero
pub(super) fn json_pointer_index(token: &str) -> Option<usize> {
    if token.starts_with('+') || (token.starts_with('0') && token.len() != 1) {
        return None;
    }
    token.parse().ok()
}

// chars

pub(super) enum CharOut {
    Bool(bool),
    Char(char),
    Str(String),
    /// `to_digit`, a u32 payload
    OptU32(Option<u32>),
    USize(usize),
}

pub(super) fn char_method(ch: char, name: BuiltinId, args: &impl Args) -> Option<Result<CharOut>> {
    let b = |v: bool| Some(Ok(CharOut::Bool(v)));
    match name {
        BuiltinId::ToDigit => {
            let radix = match int_arg(args, 0) {
                Ok(radix) => radix,
                Err(error) => return Some(Err(error)),
            };
            if !(2..=36).contains(&radix) {
                return Some(Err(anyhow!(
                    "to_digit: invalid radix -- radix must be in the range 2 to 36 inclusive"
                )));
            }
            Some(Ok(CharOut::OptU32(
                u32::try_from(radix).ok().and_then(|r| ch.to_digit(r)),
            )))
        }
        BuiltinId::IsAsciiDigit => b(ch.is_ascii_digit()),
        BuiltinId::IsAsciiAlphabetic => b(ch.is_ascii_alphabetic()),
        BuiltinId::IsAsciiAlphanumeric => b(ch.is_ascii_alphanumeric()),
        BuiltinId::IsAsciiUppercase => b(ch.is_ascii_uppercase()),
        BuiltinId::IsAsciiLowercase => b(ch.is_ascii_lowercase()),
        BuiltinId::IsAsciiWhitespace => b(ch.is_ascii_whitespace()),
        BuiltinId::IsAsciiPunctuation => b(ch.is_ascii_punctuation()),
        BuiltinId::IsAsciiHexdigit => b(ch.is_ascii_hexdigit()),
        BuiltinId::IsAscii => b(ch.is_ascii()),
        BuiltinId::IsControl => b(ch.is_control()),
        BuiltinId::EqIgnoreAsciiCase => b(args
            .text(0)
            .chars()
            .next()
            .is_some_and(|other| ch.eq_ignore_ascii_case(&other))),
        BuiltinId::LenUtf8 => Some(Ok(CharOut::USize(ch.len_utf8()))),
        BuiltinId::IsAlphabetic => b(ch.is_alphabetic()),
        BuiltinId::IsAlphanumeric => b(ch.is_alphanumeric()),
        BuiltinId::IsNumeric => b(ch.is_numeric()),
        BuiltinId::IsWhitespace => b(ch.is_whitespace()),
        BuiltinId::IsUppercase => b(ch.is_uppercase()),
        BuiltinId::IsLowercase => b(ch.is_lowercase()),
        BuiltinId::ToAsciiUppercase => Some(Ok(CharOut::Char(ch.to_ascii_uppercase()))),
        BuiltinId::ToAsciiLowercase => Some(Ok(CharOut::Char(ch.to_ascii_lowercase()))),
        // an iterator in real Rust, but a script only prints or collects it
        BuiltinId::ToUppercase => Some(Ok(CharOut::Str(ch.to_uppercase().to_string()))),
        BuiltinId::ToLowercase => Some(Ok(CharOut::Str(ch.to_lowercase().to_string()))),
        _ => None,
    }
}

// strings

/// `Keep` and `OkKeep` give the receiver back, a refcount bump and no copy.
pub(super) enum StrOut {
    Bool(bool),
    /// with the real `usize` width
    USize(usize),
    Owned(String),
    Keep,
    OkKeep,
    Strs(Vec<String>),
    CharIdx(Vec<(i64, char)>),
    Ints(Vec<i64>),
    OptOwned(Option<String>),
    OptInt(Option<i64>),
    OptPair(Option<(String, String)>),
    Ordering(Ordering),
}

/// `repeat` past `isize::MAX` must be a script panic, not an interpreter death with a different
/// exit code. A count too large for `usize` is a huge count, not zero.
fn str_repeat(s: &str, args: &impl Args) -> Result<String> {
    let n = args
        .int(0)
        .map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX));
    if s.len().saturating_mul(n) > isize::MAX.cast_unsigned() {
        bail!("capacity overflow");
    }
    Ok(s.repeat(n))
}

/// Int first, then float, then bool.
pub(super) fn str_core(s: &str, name: BuiltinId, args: &impl Args) -> Result<Option<StrOut>> {
    let a = |i: usize| args.text(i);
    Ok(Some(match name {
        BuiltinId::Len => StrOut::USize(s.len()),
        BuiltinId::IsEmpty => StrOut::Bool(s.is_empty()),
        BuiltinId::IsCharBoundary => StrOut::Bool(s.is_char_boundary(usize_arg(args, 0)?)),
        BuiltinId::IsAscii => StrOut::Bool(s.is_ascii()),
        BuiltinId::Count => StrOut::USize(s.chars().count()),
        BuiltinId::Contains => StrOut::Bool(s.contains(&a(0))),
        BuiltinId::EqIgnoreAsciiCase => StrOut::Bool(s.eq_ignore_ascii_case(&a(0))),
        BuiltinId::StartsWith => StrOut::Bool(s.starts_with(&a(0))),
        BuiltinId::EndsWith => StrOut::Bool(s.ends_with(&a(0))),
        BuiltinId::Trim => StrOut::Owned(s.trim().to_string()),
        BuiltinId::TrimStart => StrOut::Owned(s.trim_start().to_string()),
        BuiltinId::TrimEnd => StrOut::Owned(s.trim_end().to_string()),
        BuiltinId::ToUppercase => StrOut::Owned(s.to_uppercase()),
        BuiltinId::ToLowercase => StrOut::Owned(s.to_lowercase()),
        // the ascii variants leave non ascii chars alone
        BuiltinId::ToAsciiUppercase => StrOut::Owned(s.to_ascii_uppercase()),
        BuiltinId::ToAsciiLowercase => StrOut::Owned(s.to_ascii_lowercase()),
        // A char set pattern like `[':', '.']` replaces any member. Otherwise the array renders
        // as text and matches nothing.
        BuiltinId::Replace => match args.pattern_chars(0) {
            Some(cs) => StrOut::Owned(s.replace(cs.as_slice(), &a(1))),
            None => StrOut::Owned(s.replace(&a(0), &a(1))),
        },
        BuiltinId::Replacen => match args.pattern_chars(0) {
            Some(cs) => StrOut::Owned(s.replacen(cs.as_slice(), &a(1), usize_arg(args, 2)?)),
            None => StrOut::Owned(s.replacen(&a(0), &a(1), usize_arg(args, 2)?)),
        },
        BuiltinId::Repeat => StrOut::Owned(str_repeat(s, args)?),
        // A json string is a plain Str, so `unwrap` and `expect` on a string are identity. Keeps
        // serde chains working.
        BuiltinId::ToOwned
        | BuiltinId::TrimString
        | BuiltinId::AsStr
        | BuiltinId::AsString
        | BuiltinId::Unwrap
        | BuiltinId::Expect
        | BuiltinId::UnwrapOr
        | BuiltinId::UnwrapOrElse
        | BuiltinId::UnwrapOrDefault
        | BuiltinId::IntoOwned
        | BuiltinId::IntoString => StrOut::Keep,
        // `Option::context` returns a Result, otherwise a following `?` has nothing to unwrap
        BuiltinId::Context | BuiltinId::WithContext => StrOut::OkKeep,
        BuiltinId::IsSome => StrOut::Bool(true),
        BuiltinId::IsNone => StrOut::Bool(false),
        BuiltinId::AsBytes | BuiltinId::IntoBytes => {
            StrOut::Ints(s.bytes().map(i64::from).collect())
        }
        BuiltinId::EncodeUtf16 => StrOut::Ints(s.encode_utf16().map(i64::from).collect()),
        BuiltinId::StripPrefix => StrOut::OptOwned(s.strip_prefix(&a(0)).map(str::to_string)),
        BuiltinId::StripSuffix => StrOut::OptOwned(s.strip_suffix(&a(0)).map(str::to_string)),
        // byte offsets like std, so `&s[..s.find(x).unwrap()]` works
        BuiltinId::Find => StrOut::OptInt(s.find(&a(0)).map(usize_i64)),
        BuiltinId::Rfind => StrOut::OptInt(s.rfind(&a(0)).map(usize_i64)),
        BuiltinId::SplitOnce => StrOut::OptPair(
            s.split_once(&a(0))
                .map(|(x, y)| (x.to_string(), y.to_string())),
        ),
        BuiltinId::RsplitOnce => StrOut::OptPair(
            s.rsplit_once(&a(0))
                .map(|(x, y)| (x.to_string(), y.to_string())),
        ),
        // a char array splits on any of its members
        BuiltinId::Split => match args.pattern_chars(0) {
            Some(chars) => StrOut::Strs(
                s.split(|c: char| chars.contains(&c))
                    .map(str::to_string)
                    .collect(),
            ),
            None => StrOut::Strs(s.split(&a(0)).map(str::to_string).collect()),
        },
        BuiltinId::Rsplit => StrOut::Strs(s.rsplit(&a(0)).map(str::to_string).collect()),
        BuiltinId::Splitn => {
            let n = usize_arg(args, 0)?;
            StrOut::Strs(s.splitn(n, &a(1)).map(str::to_string).collect())
        }
        BuiltinId::Rsplitn => {
            let n = usize_arg(args, 0)?;
            StrOut::Strs(s.rsplitn(n, &a(1)).map(str::to_string).collect())
        }
        BuiltinId::Matches => StrOut::Strs(s.matches(&a(0)).map(str::to_string).collect()),
        BuiltinId::CharIndices => {
            StrOut::CharIdx(s.char_indices().map(|(i, c)| (usize_i64(i), c)).collect())
        }
        BuiltinId::TrimMatches | BuiltinId::TrimStartMatches | BuiltinId::TrimEndMatches => {
            let pat = a(0);
            let out = match name {
                BuiltinId::TrimStartMatches => s.trim_start_matches(&pat),
                BuiltinId::TrimEndMatches => s.trim_end_matches(&pat),
                // `trim_matches` only takes chars
                _ => match args.pattern_chars(0) {
                    Some(chars) => s.trim_matches(|c: char| chars.contains(&c)),
                    None => s.trim_matches(pat.chars().next().unwrap_or(' ')),
                },
            };
            StrOut::Owned(out.to_string())
        }
        BuiltinId::Cmp => StrOut::Ordering(s.cmp(a(0).as_str())),
        // `parse_core` is the only place that knows the target type
        _ => return Ok(None),
    }))
}

pub(super) enum Parsed {
    Int(i128, IntWidth),
    F32(f32),
    F64(f64),
    Bool(bool),
    Char(char),
    Str(String),
    Fail(String),
}

/// Written out by hand because std has no way to build a `ParseIntError`. Every other parse
/// failure carries the real error.
fn out_of_range(too_small: bool) -> String {
    if too_small {
        "number too small to fit in target type".to_string()
    } else {
        "number too large to fit in target type".to_string()
    }
}

fn int_error(text: &str) -> String {
    text.parse::<i64>()
        .err()
        .map_or_else(|| format!("cannot parse `{text}`"), |e| e.to_string())
}

/// Uses the target type when the call wrote one. Guessing made `"300".parse::<u8>()` an
/// `Ok(300)`. Without a turbofish we still guess.
pub(super) fn parse_core(text: &str, target: Option<&ScalarTy>) -> Parsed {
    let fail = |e: &dyn std::fmt::Display| Parsed::Fail(e.to_string());
    let Some(target) = target else {
        let trimmed = text.trim();
        return if let Ok(value) = trimmed.parse::<i64>() {
            Parsed::Int(i128::from(value), IntWidth::I64)
        } else if let Ok(value) = trimmed.parse::<u128>() {
            // an integer past i64 keeps its digits at 128 bits
            Parsed::Int(value.cast_signed(), IntWidth::U128)
        } else if let Ok(value) = trimmed.parse::<i128>() {
            Parsed::Int(value, IntWidth::I128)
        } else if let Ok(value) = trimmed.parse::<f64>() {
            Parsed::F64(value)
        } else if let Ok(value) = trimmed.parse::<bool>() {
            Parsed::Bool(value)
        } else {
            // report what an integer parse would say, the real std message
            Parsed::Fail(int_error(trimmed))
        };
    };
    match target {
        ScalarTy::Int(IntWidth::U128) => {
            match parse_int_digits::<u128>(text, false, 0, u128::MAX) {
                Ok(value) => Parsed::Int(value.cast_signed(), IntWidth::U128),
                Err(message) => Parsed::Fail(message),
            }
        }
        ScalarTy::Int(width) => {
            match parse_int_digits::<i128>(text, width.is_signed(), width.min(), width.max()) {
                Ok(value) => Parsed::Int(value, *width),
                Err(message) => Parsed::Fail(message),
            }
        }
        ScalarTy::F32 => text.parse::<f32>().map_or_else(|e| fail(&e), Parsed::F32),
        ScalarTy::F64 => text.parse::<f64>().map_or_else(|e| fail(&e), Parsed::F64),
        ScalarTy::Bool => text.parse::<bool>().map_or_else(|e| fail(&e), Parsed::Bool),
        ScalarTy::Char => text.parse::<char>().map_or_else(|e| fail(&e), Parsed::Char),
        ScalarTy::Str => Parsed::Str(text.to_string()),
        // no container implements `FromStr`, these only describe a `Default`
        ScalarTy::Opt(_)
        | ScalarTy::List(_)
        | ScalarTy::Map(_)
        | ScalarTy::Set(_)
        | ScalarTy::Other => Parsed::Fail(format!("cannot parse `{text}`")),
    }
}

// regex

/// Spans index into the source the caller holds.
pub(super) enum RegexOut {
    Bool(bool),
    Text(String),
    Pattern,
    OptSpan(Option<(usize, usize)>),
    OptGroups(Option<Vec<Option<(usize, usize)>>>),
    Pieces(Vec<String>),
}

/// The eager methods. `find_iter` and `captures_iter` are lazy and live elsewhere.
pub(super) fn regex_core(
    re: &regex::Regex,
    name: BuiltinId,
    source: &str,
    replacement: &dyn Fn() -> String,
) -> Option<RegexOut> {
    Some(match name {
        BuiltinId::IsMatch => RegexOut::Bool(re.is_match(source)),
        BuiltinId::Find => RegexOut::OptSpan(re.find(source).map(|m| (m.start(), m.end()))),
        BuiltinId::Captures => RegexOut::OptGroups(re.captures(source).map(|c| {
            (0..c.len())
                .map(|i| c.get(i).map(|g| (g.start(), g.end())))
                .collect()
        })),
        BuiltinId::Replace => {
            RegexOut::Text(re.replacen(source, 1, replacement().as_str()).into_owned())
        }
        BuiltinId::ReplaceAll => {
            RegexOut::Text(re.replace_all(source, replacement().as_str()).into_owned())
        }
        BuiltinId::Split => RegexOut::Pieces(re.split(source).map(str::to_string).collect()),
        BuiltinId::AsStr => RegexOut::Pattern,
        _ => return None,
    })
}

pub(super) enum MatchOut {
    Text(String),
    Int(i64),
}

pub(super) fn match_core(
    name: BuiltinId,
    source: &str,
    start: usize,
    end: usize,
) -> Option<MatchOut> {
    Some(match name {
        BuiltinId::AsStr => MatchOut::Text(source[start..end].to_string()),
        BuiltinId::Start => MatchOut::Int(usize_i64(start)),
        BuiltinId::End => MatchOut::Int(usize_i64(end)),
        _ => return None,
    })
}

pub(super) enum CapturesOut {
    Int(i64),
    OptSpan(Option<(usize, usize)>),
}

pub(super) fn captures_core<'n>(
    name: BuiltinId,
    groups: &[Option<(usize, usize)>],
    mut names: impl Iterator<Item = (&'n str, usize)>,
    args: &impl Args,
) -> Result<Option<CapturesOut>> {
    Ok(Some(match name {
        BuiltinId::Get => {
            let Some(index) = args.int(0).and_then(|i| usize::try_from(i).ok()) else {
                bail!("captures get needs a non-negative index");
            };
            CapturesOut::OptSpan(groups.get(index).copied().flatten())
        }
        BuiltinId::Name => {
            let wanted = args.text(0);
            let index = names.find_map(|(n, i)| (n == wanted).then_some(i));
            CapturesOut::OptSpan(index.and_then(|i| groups.get(i).copied().flatten()))
        }
        BuiltinId::Len => CapturesOut::Int(usize_i64(groups.len())),
        _ => return Ok(None),
    }))
}

// duration

pub(super) enum DurOut {
    Int(i64),
    Float(f64),
    Bool(bool),
}

/// The checked std ops with the real panic messages.
pub(super) fn duration_arith(op: BinKind, a: Duration, b: Duration) -> Result<Duration> {
    match op {
        BinKind::Add => a
            .checked_add(b)
            .ok_or_else(|| anyhow!("overflow when adding durations")),
        BinKind::Sub => a
            .checked_sub(b)
            .ok_or_else(|| anyhow!("overflow when subtracting durations")),
        _ => bail!("cannot apply that operator to two durations"),
    }
}

pub(super) fn duration_core(name: BuiltinId, secs: u64, nanos: u32) -> Option<DurOut> {
    let total = u128::from(secs) * 1_000_000_000 + u128::from(nanos);
    Some(match name {
        BuiltinId::AsSecs => DurOut::Int(i64::try_from(secs).unwrap_or(i64::MAX)),
        BuiltinId::AsMillis => DurOut::Int(i64::try_from(total / 1_000_000).unwrap_or(i64::MAX)),
        BuiltinId::AsMicros => DurOut::Int(i64::try_from(total / 1_000).unwrap_or(i64::MAX)),
        BuiltinId::AsNanos => DurOut::Int(i64::try_from(total).unwrap_or(i64::MAX)),
        BuiltinId::SubsecNanos => DurOut::Int(i64::from(nanos)),
        BuiltinId::SubsecMillis => DurOut::Int(i64::from(nanos / 1_000_000)),
        BuiltinId::SubsecMicros => DurOut::Int(i64::from(nanos / 1_000)),
        BuiltinId::AsSecsF64 => {
            DurOut::Float(AsPrimitive::<f64>::as_(secs) + f64::from(nanos) / 1e9)
        }
        BuiltinId::IsZero => DurOut::Bool(total == 0),
        _ => return None,
    })
}

// datetime

pub(super) enum DateOut {
    Int(i64),
    Text(String),
}

/// `parse_from_rfc3339` reduced to unix seconds, nanos and the offset. The error is the real
/// chrono message.
pub(super) fn parse_rfc3339(text: &str) -> Result<(i64, u32, i32), String> {
    use chrono::{DateTime, Offset, Timelike};
    match DateTime::parse_from_rfc3339(text) {
        Ok(dt) => Ok((
            dt.timestamp(),
            dt.nanosecond(),
            dt.offset().fix().local_minus_utc(),
        )),
        Err(e) => Err(e.to_string()),
    }
}

/// `local` picks the machine timezone, otherwise the value is read through `offset`. A calendar field
/// is read in the zone the value carries, like in real chrono.
pub(super) fn datetime_core(
    name: BuiltinId,
    secs: i64,
    nanos: u32,
    local: bool,
    offset: i32,
    args: &impl Args,
) -> Option<DateOut> {
    use chrono::{DateTime, Datelike, FixedOffset, Local, Offset, Timelike, Utc};
    let utc: DateTime<Utc> = DateTime::from_timestamp(secs, nanos).unwrap_or_default();
    let view = if local {
        utc.with_timezone(&Local).fixed_offset()
    } else {
        utc.with_timezone(&FixedOffset::east_opt(offset).unwrap_or(Utc.fix()))
    };
    Some(match name {
        BuiltinId::Timestamp => DateOut::Int(secs),
        BuiltinId::TimestampMillis => DateOut::Int(secs * 1000 + i64::from(nanos / 1_000_000)),
        BuiltinId::ToRfc3339 => DateOut::Text(view.to_rfc3339()),
        BuiltinId::Format => DateOut::Text(view.format(&args.text(0)).to_string()),
        BuiltinId::Year => DateOut::Int(i64::from(view.year())),
        BuiltinId::Month => DateOut::Int(i64::from(view.month())),
        BuiltinId::Day => DateOut::Int(i64::from(view.day())),
        BuiltinId::Hour => DateOut::Int(i64::from(view.hour())),
        BuiltinId::Minute => DateOut::Int(i64::from(view.minute())),
        BuiltinId::Second => DateOut::Int(i64::from(view.second())),
        _ => return None,
    })
}

// http and process scalars

pub(super) enum StatusOut {
    Int(i64),
    Bool(bool),
}

pub(super) fn status_core(name: BuiltinId, code: i64) -> Option<StatusOut> {
    Some(match name {
        BuiltinId::AsU16 | BuiltinId::AsInt => StatusOut::Int(code),
        BuiltinId::IsSuccess => StatusOut::Bool((200..300).contains(&code)),
        BuiltinId::IsClientError => StatusOut::Bool((400..500).contains(&code)),
        BuiltinId::IsServerError => StatusOut::Bool((500..600).contains(&code)),
        _ => return None,
    })
}

pub(super) enum HeaderOut {
    /// `to_str` gives `Ok(text)` like the real fallible accessor
    Ok(String),
    Text(String),
}

pub(super) fn header_value_core(name: BuiltinId, text: String) -> Option<HeaderOut> {
    Some(match name {
        BuiltinId::ToStr => HeaderOut::Ok(text),
        BuiltinId::AsStr | BuiltinId::AsString | BuiltinId::ToString => HeaderOut::Text(text),
        _ => return None,
    })
}

pub(super) enum ExitOut {
    Bool(bool),
    /// `None` after death by signal
    OptInt(Option<i64>),
}

pub(super) fn exit_status_core(
    name: BuiltinId,
    success: bool,
    code: Option<i64>,
) -> Option<ExitOut> {
    Some(match name {
        BuiltinId::Success => ExitOut::Bool(success),
        BuiltinId::Code => ExitOut::OptInt(code),
        _ => return None,
    })
}

/// The `colored` crate as string methods. Returns a plain string with ANSI codes so chaining
/// works. Honors `NO_COLOR` and terminal detection.
pub(super) fn color_core(s: &str, name: BuiltinId) -> Option<String> {
    use colored::Colorize;
    let out = match name {
        BuiltinId::Red => s.red(),
        BuiltinId::Green => s.green(),
        BuiltinId::Yellow => s.yellow(),
        BuiltinId::Blue => s.blue(),
        BuiltinId::Magenta | BuiltinId::Purple => s.magenta(),
        BuiltinId::Cyan => s.cyan(),
        BuiltinId::White => s.white(),
        BuiltinId::Black => s.black(),
        BuiltinId::BrightRed => s.bright_red(),
        BuiltinId::BrightGreen => s.bright_green(),
        BuiltinId::BrightYellow => s.bright_yellow(),
        BuiltinId::BrightBlue => s.bright_blue(),
        BuiltinId::BrightCyan => s.bright_cyan(),
        BuiltinId::OnRed => s.on_red(),
        BuiltinId::OnGreen => s.on_green(),
        BuiltinId::OnBlue => s.on_blue(),
        BuiltinId::Bold => s.bold(),
        BuiltinId::Dimmed => s.dimmed(),
        BuiltinId::Italic => s.italic(),
        BuiltinId::Underline => s.underline(),
        BuiltinId::Reversed => s.reversed(),
        BuiltinId::Clear | BuiltinId::Normal => s.normal(),
        _ => return None,
    };
    Some(out.to_string())
}

/// The digit loop of `from_str_radix`, so the first error in reading order wins the way it does
/// in std. `"99999999999999999999\n".parse::<usize>()` overflows before the newline is seen, and
/// `"-1".parse::<u8>()` rejects the sign as a digit. The bounds stand in for the target width.
fn parse_int_digits<T: PrimInt>(text: &str, signed: bool, low: T, high: T) -> Result<T, String> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return Err("cannot parse integer from empty string".to_string());
    }
    let (positive, digits) = match bytes {
        [b'+' | b'-'] => return Err("invalid digit found in string".to_string()),
        [b'+', rest @ ..] => (true, rest),
        [b'-', rest @ ..] if signed => (false, rest),
        _ => (true, bytes),
    };
    let overflow = || Err(out_of_range(!positive));
    let radix = T::from(10).expect("10 fits every integer");
    let mut result = T::zero();
    for byte in digits {
        let scaled = result
            .checked_mul(&radix)
            .filter(|v| *v >= low && *v <= high);
        let Some(digit) = (*byte as char).to_digit(10) else {
            return Err("invalid digit found in string".to_string());
        };
        let Some(scaled) = scaled else {
            return overflow();
        };
        let digit = T::from(digit).expect("a digit fits every integer");
        let next = if positive {
            scaled.checked_add(&digit)
        } else {
            scaled.checked_sub(&digit)
        };
        match next.filter(|v| *v >= low && *v <= high) {
            Some(next) => result = next,
            None => return overflow(),
        }
    }
    Ok(result)
}
