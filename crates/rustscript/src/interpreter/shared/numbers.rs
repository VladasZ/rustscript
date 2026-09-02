//! The numeric method cores, `f64` first with an exact `f32` twin.

use num_traits::AsPrimitive;
use std::cmp::Ordering;

use anyhow::{Result, bail};

use crate::interpreter::bytecode::BuiltinId;

use super::{Args, float_arg, int_arg};

#[derive(Clone, Copy)]
pub(crate) enum Num {
    Int(i64),
    Float(f64),
}

pub(crate) enum NumOut {
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

pub(crate) fn num_core(recv: Num, name: BuiltinId, args: &impl Args) -> Result<Option<NumOut>> {
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

pub(crate) enum F32Out {
    Val(f32),
    Bool(bool),
    Bytes(Vec<u8>),
    Ordering(Ordering),
    SomeOrdering(Ordering),
}

/// Computed in real f32. Through the f64 core `sqrt` double rounds and `{:?}` prints
/// `3.4028234663852886e38` instead of `3.4028235e38`.
pub(crate) fn f32_core(recv: f32, name: BuiltinId, args: &impl Args) -> Result<Option<F32Out>> {
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
