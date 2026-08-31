//! The 128 bit integer methods, stamped once per type.

use anyhow::{Result, bail};
use num_traits::AsPrimitive;

use super::{IntOut, arg, count_arg};
use crate::interpreter::bytecode::BuiltinId;
use crate::interpreter::numeric::IntWidth;

/// Stamps the native method body for 1 128 bit type. New names must go into both this and the
/// i128 pipeline, the harvest reads the pipeline.
macro_rules! big_methods {
    ($fn_name:ident, $query_name:ident, $ty:ty, $decode:expr, $encode:expr) => {
        fn $fn_name(name: BuiltinId, recv_bits: i128, args: &[i128]) -> Option<Result<IntOut>> {
            let decode = $decode;
            let encode = $encode;
            let recv: $ty = decode(recv_bits);
            let val = |i: usize| -> Result<$ty> { Ok(decode(arg(args, i)?)) };
            let same = |v: $ty| IntOut::Same(encode(v));
            let checked = |v: Option<$ty>| IntOut::Checked(v.map(encode));
            let out: Result<IntOut> = match name {
                BuiltinId::SaturatingAdd => val(0).map(|b| same(recv.saturating_add(b))),
                BuiltinId::SaturatingSub => val(0).map(|b| same(recv.saturating_sub(b))),
                BuiltinId::SaturatingMul => val(0).map(|b| same(recv.saturating_mul(b))),
                BuiltinId::WrappingAdd => val(0).map(|b| same(recv.wrapping_add(b))),
                BuiltinId::WrappingSub => val(0).map(|b| same(recv.wrapping_sub(b))),
                BuiltinId::WrappingMul => val(0).map(|b| same(recv.wrapping_mul(b))),
                BuiltinId::WrappingNeg => Ok(same(recv.wrapping_neg())),
                BuiltinId::CheckedAdd => val(0).map(|b| checked(recv.checked_add(b))),
                BuiltinId::CheckedSub => val(0).map(|b| checked(recv.checked_sub(b))),
                BuiltinId::CheckedMul => val(0).map(|b| checked(recv.checked_mul(b))),
                BuiltinId::CheckedNeg => Ok(checked(recv.checked_neg())),
                BuiltinId::CheckedDiv => val(0).map(|b| checked(recv.checked_div(b))),
                BuiltinId::CheckedRem => val(0).map(|b| checked(recv.checked_rem(b))),
                BuiltinId::CheckedShl => count_arg(args, 0).map(|n| checked(recv.checked_shl(n))),
                BuiltinId::CheckedShr => count_arg(args, 0).map(|n| checked(recv.checked_shr(n))),
                BuiltinId::Pow => count_arg(args, 0).and_then(|e| match recv.checked_pow(e) {
                    Some(v) => Ok(same(v)),
                    None => bail!("attempt to exponentiate with overflow"),
                }),
                BuiltinId::CheckedPow => count_arg(args, 0).map(|e| checked(recv.checked_pow(e))),
                BuiltinId::SaturatingPow => {
                    count_arg(args, 0).map(|e| same(recv.saturating_pow(e)))
                }
                BuiltinId::WrappingPow => count_arg(args, 0).map(|e| same(recv.wrapping_pow(e))),
                BuiltinId::WrappingShl => count_arg(args, 0).map(|n| same(recv.wrapping_shl(n))),
                BuiltinId::WrappingShr => count_arg(args, 0).map(|n| same(recv.wrapping_shr(n))),
                BuiltinId::Midpoint => val(0).map(|b| same(recv.midpoint(b))),
                BuiltinId::OverflowingAdd => val(0).map(|b| {
                    let (v, flag) = recv.overflowing_add(b);
                    IntOut::Overflowing(encode(v), flag)
                }),
                BuiltinId::OverflowingSub => val(0).map(|b| {
                    let (v, flag) = recv.overflowing_sub(b);
                    IntOut::Overflowing(encode(v), flag)
                }),
                BuiltinId::OverflowingMul => val(0).map(|b| {
                    let (v, flag) = recv.overflowing_mul(b);
                    IntOut::Overflowing(encode(v), flag)
                }),
                BuiltinId::CheckedRemEuclid => val(0).map(|b| checked(recv.checked_rem_euclid(b))),
                BuiltinId::Ilog2 => match recv.checked_ilog2() {
                    Some(v) => Ok(IntOut::Count(v)),
                    None => Err(anyhow::anyhow!(
                        "argument of integer logarithm must be positive"
                    )),
                },
                BuiltinId::Ilog10 => match recv.checked_ilog10() {
                    Some(v) => Ok(IntOut::Count(v)),
                    None => Err(anyhow::anyhow!(
                        "argument of integer logarithm must be positive"
                    )),
                },
                BuiltinId::CheckedIlog2 => Ok(IntOut::CheckedCount(recv.checked_ilog2())),
                BuiltinId::LeadingOnes => Ok(IntOut::Count(recv.leading_ones())),
                BuiltinId::TrailingOnes => Ok(IntOut::Count(recv.trailing_ones())),
                BuiltinId::DivEuclid => val(0).and_then(|b| {
                    if b == 0 {
                        bail!("attempt to divide by zero");
                    }
                    match recv.checked_div_euclid(b) {
                        Some(v) => Ok(same(v)),
                        None => bail!("attempt to divide with overflow"),
                    }
                }),
                BuiltinId::RemEuclid => val(0).and_then(|b| {
                    if b == 0 {
                        bail!("attempt to calculate the remainder with a divisor of zero");
                    }
                    match recv.checked_rem_euclid(b) {
                        Some(v) => Ok(same(v)),
                        None => bail!("attempt to calculate the remainder with overflow"),
                    }
                }),
                _ => return $query_name(name, recv_bits, args),
            };
            Some(out)
        }

        /// the comparison, bit and byte half
        fn $query_name(name: BuiltinId, recv_bits: i128, args: &[i128]) -> Option<Result<IntOut>> {
            let decode = $decode;
            let encode = $encode;
            let recv: $ty = decode(recv_bits);
            let val = |i: usize| -> Result<$ty> { Ok(decode(arg(args, i)?)) };
            let same = |v: $ty| IntOut::Same(encode(v));
            let out: Result<IntOut> = match name {
                BuiltinId::Min => val(0).map(|b| same(recv.min(b))),
                BuiltinId::Max => val(0).map(|b| same(recv.max(b))),
                BuiltinId::Clamp => val(0).and_then(|low| {
                    let high = val(1)?;
                    if low > high {
                        bail!("min > max. min = {low}, max = {high}");
                    }
                    Ok(same(recv.clamp(low, high)))
                }),
                BuiltinId::Cmp => val(0).map(|b| IntOut::Ordering(recv.cmp(&b))),
                BuiltinId::IsMultipleOf => val(0).map(|b| {
                    IntOut::Bool(match b {
                        0 => recv == 0,
                        // the only `None` remainder is `MIN % -1`, which is 0
                        _ => recv.checked_rem(b).is_none_or(|r| r == 0),
                    })
                }),
                BuiltinId::CountOnes => Ok(IntOut::Count(recv.count_ones())),
                BuiltinId::CountZeros => Ok(IntOut::Count(recv.count_zeros())),
                BuiltinId::LeadingZeros => Ok(IntOut::Count(recv.leading_zeros())),
                BuiltinId::TrailingZeros => Ok(IntOut::Count(recv.trailing_zeros())),
                BuiltinId::RotateLeft => count_arg(args, 0).map(|n| same(recv.rotate_left(n))),
                BuiltinId::RotateRight => count_arg(args, 0).map(|n| same(recv.rotate_right(n))),
                BuiltinId::SwapBytes => Ok(same(recv.swap_bytes())),
                BuiltinId::ReverseBits => Ok(same(recv.reverse_bits())),
                BuiltinId::ToLeBytes => Ok(IntOut::Bytes(recv.to_le_bytes().to_vec())),
                BuiltinId::ToBeBytes => Ok(IntOut::Bytes(recv.to_be_bytes().to_vec())),
                BuiltinId::ToNeBytes => Ok(IntOut::Bytes(recv.to_ne_bytes().to_vec())),
                BuiltinId::AsI64 => Ok(IntOut::Checked(i64::try_from(recv).ok().map(i128::from))),
                BuiltinId::AsU64 => Ok(IntOut::Checked(u64::try_from(recv).ok().map(i128::from))),
                BuiltinId::AsF64 => Ok(IntOut::SomeFloat(AsPrimitive::<f64>::as_(recv))),
                _ => return None,
            };
            Some(out)
        }
    };
}

big_methods!(
    u128_method,
    u128_query,
    u128,
    |bits: i128| bits.cast_unsigned(),
    |v: u128| { v.cast_signed() }
);
big_methods!(
    i128_method,
    i128_query,
    i128,
    |bits: i128| bits,
    |v: i128| v
);

/// The i128 pipeline can't host these, a `u128` past `i128::MAX` stores as negative bits. Payloads are
/// raw bits like `Value::Big` carries. The signed only names live here because the stamped body
/// must compile for u128 too.
pub fn big_int_method(
    name: BuiltinId,
    width: IntWidth,
    recv: i128,
    args: &[i128],
) -> Option<Result<IntOut>> {
    match width {
        IntWidth::U128 => match name {
            BuiltinId::Isqrt => Some(Ok(IntOut::Same(recv.cast_unsigned().isqrt().cast_signed()))),
            BuiltinId::IsPowerOfTwo => {
                Some(Ok(IntOut::Bool(recv.cast_unsigned().is_power_of_two())))
            }
            BuiltinId::NextPowerOfTwo => {
                Some(match recv.cast_unsigned().checked_next_power_of_two() {
                    Some(v) => Ok(IntOut::Same(v.cast_signed())),
                    None => Err(anyhow::anyhow!("attempt to add with overflow")),
                })
            }
            BuiltinId::CheckedNextPowerOfTwo => Some(Ok(IntOut::Checked(
                recv.cast_unsigned()
                    .checked_next_power_of_two()
                    .map(u128::cast_signed),
            ))),
            BuiltinId::DivCeil => Some(arg(args, 0).and_then(|b| {
                if b == 0 {
                    bail!("attempt to divide by zero");
                }
                Ok(IntOut::Same(
                    recv.cast_unsigned()
                        .div_ceil(b.cast_unsigned())
                        .cast_signed(),
                ))
            })),
            BuiltinId::NextMultipleOf => Some(arg(args, 0).and_then(|b| {
                if b == 0 {
                    bail!("attempt to calculate the remainder with a divisor of zero");
                }
                match recv
                    .cast_unsigned()
                    .checked_next_multiple_of(b.cast_unsigned())
                {
                    Some(v) => Ok(IntOut::Same(v.cast_signed())),
                    None => bail!("attempt to add with overflow"),
                }
            })),
            _ => u128_method(name, recv, args),
        },
        IntWidth::I128 => match name {
            BuiltinId::Isqrt => Some(if recv < 0 {
                Err(anyhow::anyhow!(
                    "argument of integer square root cannot be negative"
                ))
            } else {
                Ok(IntOut::Same(recv.isqrt()))
            }),
            BuiltinId::Abs => Some(if recv == i128::MIN {
                Err(anyhow::anyhow!("attempt to negate with overflow"))
            } else {
                Ok(IntOut::Same(recv.abs()))
            }),
            BuiltinId::Signum => Some(Ok(IntOut::Same(recv.signum()))),
            BuiltinId::IsPositive => Some(Ok(IntOut::Bool(recv.is_positive()))),
            BuiltinId::IsNegative => Some(Ok(IntOut::Bool(recv.is_negative()))),
            BuiltinId::WrappingAbs => Some(Ok(IntOut::Same(recv.wrapping_abs()))),
            BuiltinId::CheckedAbs => Some(Ok(IntOut::Checked(recv.checked_abs()))),
            _ => i128_method(name, recv, args),
        },
        _ => None,
    }
}
