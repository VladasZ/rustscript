//! Width aware integer methods. They once ran on the i64 image, so
//! `200u8.saturating_add(100)` answered 300 and a `u64` past `i64::MAX` was
//! clamped. The receiver arrives here with its true value and width.

use num_traits::AsPrimitive;
use std::cmp::Ordering;

use anyhow::{Result, bail};

use super::bytecode::BuiltinId;
use super::numeric::IntWidth;

pub enum IntOut {
    Same(i128),
    /// A bit count, always `u32`.
    Count(u32),
    Bool(bool),
    /// `checked_*`, and the serde `as_i64` and `as_u64`, a value only when it
    /// fits.
    Checked(Option<i128>),
    /// `as_f64`, always a `Some` in serde.
    SomeFloat(f64),
    Ordering(Ordering),
    /// `to_le_bytes` and its siblings.
    Bytes(Vec<u8>),
    /// `overflowing_*`.
    Overflowing(i128, bool),
    /// `checked_ilog2`.
    CheckedCount(Option<u32>),
}

/// `Ne` is the host's own order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ByteOrder {
    Le,
    Be,
    Ne,
}

impl ByteOrder {
    fn little(self) -> bool {
        match self {
            Self::Le => true,
            Self::Be => false,
            Self::Ne => cfg!(target_endian = "little"),
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Self::Le => "le",
            Self::Be => "be",
            Self::Ne => "ne",
        }
    }
}

/// `None` when the name is not one of the 3 byte conversions.
pub fn from_bytes_order(name: &str) -> Option<ByteOrder> {
    Some(match name {
        "from_le_bytes" => ByteOrder::Le,
        "from_be_bytes" => ByteOrder::Be,
        "from_ne_bytes" => ByteOrder::Ne,
        _ => return None,
    })
}

/// Real Rust takes an exact `[u8; N]`, so a wrong length is an error rather
/// than a guess.
pub fn from_bytes(width: IntWidth, order: ByteOrder, bytes: &[i128]) -> Result<i128> {
    let count = byte_count(width);
    if bytes.len() != count {
        bail!(
            "`{}::from_{}_bytes` needs {count} bytes, got {}",
            width.name(),
            order.tag(),
            bytes.len()
        );
    }
    let mut bits: u128 = 0;
    for (index, byte) in bytes.iter().enumerate() {
        let Ok(byte) = u8::try_from(*byte) else {
            bail!(
                "`{}::from_{}_bytes` needs bytes, got {byte}",
                width.name(),
                order.tag()
            );
        };
        let place = if order.little() {
            index
        } else {
            count - 1 - index
        };
        bits |= u128::from(byte) << (place * 8);
    }
    Ok(from_raw(width, bits))
}

fn byte_count(width: IntWidth) -> usize {
    (width.bits() / 8) as usize
}

/// Over the receiver's real width, so a signed value writes two's
/// complement.
fn to_bytes(width: IntWidth, value: i128, order: ByteOrder) -> Vec<u8> {
    let count = byte_count(width);
    let bits = raw(width, value);
    let mut out: Vec<u8> = (0..count)
        .map(|index| ((bits >> (index * 8)) & 0xff) as u8)
        .collect();
    if !order.little() {
        out.reverse();
    }
    out
}

/// A 128 bit value already stores its raw bits.
fn raw(width: IntWidth, value: i128) -> u128 {
    let bits = width.bits();
    if bits == 128 {
        return value.cast_unsigned();
    }
    let mask = (1u128 << bits) - 1;
    AsPrimitive::<u128>::as_(value) & mask
}

/// Sign extends when the width is signed. What `wrapping_*` and the bit
/// methods return through.
fn from_raw(width: IntWidth, bits_value: u128) -> i128 {
    let bits = width.bits();
    if bits == 128 {
        return bits_value.cast_signed();
    }
    let mask = (1u128 << bits) - 1;
    let truncated = bits_value & mask;
    if width.is_signed() && truncated >> (bits - 1) & 1 == 1 {
        AsPrimitive::<i128>::as_(truncated) - (1i128 << bits)
    } else {
        AsPrimitive::<i128>::as_(truncated)
    }
}

fn saturate(width: IntWidth, value: i128) -> i128 {
    value.clamp(width.min(), width.max())
}

fn in_range(width: IntWidth, value: i128) -> Option<i128> {
    (value >= width.min() && value <= width.max()).then_some(value)
}

/// Checked step by step so the panic lands where debug Rust's does. The
/// multiply is checked in i128 too, a `u64` product can pass what it holds.
fn pow(width: IntWidth, base: i128, exponent: u32) -> Result<i128> {
    let mut result: i128 = 1;
    for _ in 0..exponent {
        let Some(next) = result.checked_mul(base) else {
            bail!("attempt to multiply with overflow");
        };
        result = next;
        if result < width.min() || result > width.max() {
            bail!("attempt to multiply with overflow");
        }
    }
    Ok(result)
}

fn arg(args: &[i128], index: usize) -> Result<i128> {
    match args.get(index) {
        Some(value) => Ok(*value),
        None => bail!("missing argument"),
    }
}

/// A shift or rotate amount is a `u32`.
fn count_arg(args: &[i128], index: usize) -> Result<u32> {
    let value = arg(args, index)?;
    match u32::try_from(value) {
        Ok(count) => Ok(count),
        Err(_) => bail!("shift amount does not fit u32"),
    }
}

/// Methods whose argument is a `u32` amount, so the dispatch must not unify
/// the receiver's width with it.
pub fn takes_amount_arg(name: BuiltinId) -> bool {
    matches!(
        name,
        BuiltinId::Pow
            | BuiltinId::Powi
            | BuiltinId::CheckedPow
            | BuiltinId::SaturatingPow
            | BuiltinId::WrappingPow
            | BuiltinId::WrappingShl
            | BuiltinId::WrappingShr
            | BuiltinId::RotateLeft
            | BuiltinId::RotateRight
            | BuiltinId::CheckedShl
            | BuiltinId::CheckedShr
    )
}

pub fn int_method(
    name: BuiltinId,
    width: IntWidth,
    recv: i128,
    args: &[i128],
) -> Option<Result<IntOut>> {
    int_arith_method(name, width, recv, args).or_else(|| int_query_method(name, width, recv, args))
}

/// Stamps the native method body for one 128 bit type. New names must go
/// into both this and the i128 pipeline, the harvest reads the pipeline.
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
                    None => bail!("attempt to multiply with overflow"),
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

        /// The comparison, bit and byte half.
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
                        // The only `None` remainder is `MIN % -1`, which is 0.
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

/// The i128 pipeline cannot host these, a `u128` past `i128::MAX` stores as
/// negative bits. Payloads are raw bits like `Value::Big` carries. The
/// signed only names live here because the stamped body must compile for
/// u128 too.
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

fn int_arith_method(
    name: BuiltinId,
    width: IntWidth,
    recv: i128,
    args: &[i128],
) -> Option<Result<IntOut>> {
    if let Some(out) = int_wrapping_family(name, width, recv, args)
        .or_else(|| int_checked_family(name, width, recv, args))
    {
        return Some(out);
    }
    let out = match name {
        BuiltinId::SaturatingAdd => {
            arg(args, 0).map(|b| IntOut::Same(saturate(width, recv.saturating_add(b))))
        }
        BuiltinId::SaturatingSub => {
            arg(args, 0).map(|b| IntOut::Same(saturate(width, recv.saturating_sub(b))))
        }
        BuiltinId::SaturatingMul => {
            arg(args, 0).map(|b| IntOut::Same(saturate(width, recv.saturating_mul(b))))
        }
        BuiltinId::WrappingAdd => arg(args, 0).map(|b| {
            IntOut::Same(from_raw(
                width,
                AsPrimitive::<u128>::as_(recv.wrapping_add(b)),
            ))
        }),
        BuiltinId::WrappingSub => arg(args, 0).map(|b| {
            IntOut::Same(from_raw(
                width,
                AsPrimitive::<u128>::as_(recv.wrapping_sub(b)),
            ))
        }),
        BuiltinId::WrappingMul => arg(args, 0).map(|b| {
            IntOut::Same(from_raw(
                width,
                AsPrimitive::<u128>::as_(recv.wrapping_mul(b)),
            ))
        }),
        BuiltinId::WrappingNeg => Ok(IntOut::Same(from_raw(
            width,
            AsPrimitive::<u128>::as_(-recv),
        ))),
        BuiltinId::Pow => count_arg(args, 0).and_then(|e| pow(width, recv, e).map(IntOut::Same)),
        BuiltinId::CheckedPow => {
            count_arg(args, 0).map(|e| IntOut::Checked(pow(width, recv, e).ok()))
        }
        BuiltinId::WrappingAbs => {
            if !width.is_signed() {
                return None;
            }
            Ok(IntOut::Same(if recv == width.min() {
                recv
            } else {
                recv.abs()
            }))
        }
        BuiltinId::CheckedAbs => {
            if !width.is_signed() {
                return None;
            }
            Ok(IntOut::Checked((recv != width.min()).then(|| recv.abs())))
        }
        BuiltinId::Abs => {
            if !width.is_signed() {
                return None;
            }
            if recv == width.min() {
                Err(anyhow::anyhow!("attempt to negate with overflow"))
            } else {
                Ok(IntOut::Same(recv.abs()))
            }
        }
        BuiltinId::Signum => {
            if !width.is_signed() {
                return None;
            }
            Ok(IntOut::Same(recv.signum()))
        }
        _ => return None,
    };
    Some(out)
}

fn int_checked_family(
    name: BuiltinId,
    width: IntWidth,
    recv: i128,
    args: &[i128],
) -> Option<Result<IntOut>> {
    let bits = width.bits();
    let out = match name {
        BuiltinId::CheckedAdd => arg(args, 0)
            .map(|b| IntOut::Checked(recv.checked_add(b).and_then(|v| in_range(width, v)))),
        BuiltinId::CheckedSub => arg(args, 0)
            .map(|b| IntOut::Checked(recv.checked_sub(b).and_then(|v| in_range(width, v)))),
        BuiltinId::CheckedMul => arg(args, 0)
            .map(|b| IntOut::Checked(recv.checked_mul(b).and_then(|v| in_range(width, v)))),
        BuiltinId::CheckedNeg => Ok(IntOut::Checked(in_range(width, -recv))),
        BuiltinId::CheckedDiv => arg(args, 0).map(|b| {
            IntOut::Checked(if b == 0 {
                None
            } else {
                in_range(width, recv / b)
            })
        }),
        BuiltinId::CheckedRem => arg(args, 0).map(|b| {
            // `MIN % -1` overflows in the receiver's width even though the
            // i128 remainder is 0.
            IntOut::Checked(
                if b == 0 || (width.is_signed() && b == -1 && recv == width.min()) {
                    None
                } else {
                    in_range(width, recv % b)
                },
            )
        }),
        // A shift is checked on the amount alone.
        BuiltinId::CheckedShl => count_arg(args, 0)
            .map(|n| IntOut::Checked((n < bits).then(|| from_raw(width, raw(width, recv) << n)))),
        BuiltinId::CheckedShr => count_arg(args, 0).map(|n| {
            IntOut::Checked((n < bits).then(|| {
                if width.is_signed() {
                    recv >> n
                } else {
                    from_raw(width, raw(width, recv) >> n)
                }
            }))
        }),
        _ => return None,
    };
    Some(out)
}

fn int_wrapping_family(
    name: BuiltinId,
    width: IntWidth,
    recv: i128,
    args: &[i128],
) -> Option<Result<IntOut>> {
    let bits = width.bits();
    let out = match name {
        BuiltinId::SaturatingPow => count_arg(args, 0).map(|e| {
            IntOut::Same(match pow(width, recv, e) {
                Ok(v) => v,
                Err(_) if recv < 0 && e % 2 == 1 => width.min(),
                Err(_) => width.max(),
            })
        }),
        BuiltinId::WrappingPow => count_arg(args, 0).map(|e| {
            let mask = if bits == 128 {
                u128::MAX
            } else {
                (1u128 << bits) - 1
            };
            let base = raw(width, recv);
            let mut acc: u128 = 1;
            for _ in 0..e {
                acc = acc.wrapping_mul(base) & mask;
            }
            IntOut::Same(from_raw(width, acc))
        }),
        BuiltinId::WrappingShl => count_arg(args, 0)
            .map(|n| IntOut::Same(from_raw(width, raw(width, recv) << (n % bits)))),
        BuiltinId::WrappingShr => count_arg(args, 0).map(|n| {
            IntOut::Same(if width.is_signed() {
                recv >> (n % bits)
            } else {
                from_raw(width, raw(width, recv) >> (n % bits))
            })
        }),
        BuiltinId::OverflowingAdd | BuiltinId::OverflowingSub | BuiltinId::OverflowingMul => {
            arg(args, 0).map(|b| {
                let exact = match name {
                    BuiltinId::OverflowingAdd => recv + b,
                    BuiltinId::OverflowingSub => recv - b,
                    _ => recv * b,
                };
                match in_range(width, exact) {
                    Some(v) => IntOut::Overflowing(v, false),
                    None => {
                        IntOut::Overflowing(from_raw(width, AsPrimitive::<u128>::as_(exact)), true)
                    }
                }
            })
        }
        _ => return None,
    };
    Some(out)
}

/// Some of these exist on one signedness only.
fn int_range_family(
    name: BuiltinId,
    width: IntWidth,
    recv: i128,
    args: &[i128],
) -> Option<Result<IntOut>> {
    let out = match name {
        BuiltinId::Ilog2 | BuiltinId::Ilog10 => {
            if recv <= 0 {
                Err(anyhow::anyhow!(
                    "argument of integer logarithm must be positive"
                ))
            } else if name == BuiltinId::Ilog2 {
                Ok(IntOut::Count(recv.ilog2()))
            } else {
                Ok(IntOut::Count(recv.ilog10()))
            }
        }
        BuiltinId::CheckedIlog2 => Ok(IntOut::CheckedCount(recv.checked_ilog2())),
        BuiltinId::IsPositive => {
            if !width.is_signed() {
                return None;
            }
            Ok(IntOut::Bool(recv > 0))
        }
        BuiltinId::IsNegative => {
            if !width.is_signed() {
                return None;
            }
            Ok(IntOut::Bool(recv < 0))
        }
        BuiltinId::IsPowerOfTwo => {
            if width.is_signed() {
                return None;
            }
            Ok(IntOut::Bool(raw(width, recv).is_power_of_two()))
        }
        BuiltinId::NextPowerOfTwo => {
            if width.is_signed() {
                return None;
            }
            let value = raw(width, recv);
            let next = if value <= 1 {
                1
            } else {
                1u128 << (128 - (value - 1).leading_zeros())
            };
            match in_range(width, AsPrimitive::<i128>::as_(next)) {
                Some(v) => Ok(IntOut::Same(v)),
                None => Err(anyhow::anyhow!("attempt to add with overflow")),
            }
        }
        BuiltinId::DivCeil => {
            if width.is_signed() {
                return None;
            }
            arg(args, 0).and_then(|b| {
                if b == 0 {
                    bail!("attempt to divide by zero");
                }
                Ok(IntOut::Same((recv + b - 1) / b))
            })
        }
        BuiltinId::NextMultipleOf => {
            if width.is_signed() {
                return None;
            }
            arg(args, 0).and_then(|b| {
                if b == 0 {
                    bail!("attempt to calculate the remainder with a divisor of zero");
                }
                let rem = recv % b;
                let next = if rem == 0 { recv } else { recv + (b - rem) };
                match in_range(width, next) {
                    Some(v) => Ok(IntOut::Same(v)),
                    None => bail!("attempt to add with overflow"),
                }
            })
        }
        _ => return None,
    };
    Some(out)
}

/// All over the raw bits of the receiver's width.
fn int_bit_family(
    name: BuiltinId,
    width: IntWidth,
    recv: i128,
    args: &[i128],
) -> Option<Result<IntOut>> {
    let bits = width.bits();
    let out = match name {
        BuiltinId::CountOnes => Ok(IntOut::Count(raw(width, recv).count_ones())),
        BuiltinId::CountZeros => Ok(IntOut::Count(bits - raw(width, recv).count_ones())),
        BuiltinId::LeadingZeros => {
            let value = raw(width, recv);
            Ok(IntOut::Count(if value == 0 {
                bits
            } else {
                value.leading_zeros() - (128 - bits)
            }))
        }
        BuiltinId::TrailingZeros => {
            let value = raw(width, recv);
            Ok(IntOut::Count(if value == 0 {
                bits
            } else {
                value.trailing_zeros()
            }))
        }
        BuiltinId::RotateLeft => {
            count_arg(args, 0).map(|n| IntOut::Same(rotate(width, recv, n, true)))
        }
        BuiltinId::RotateRight => {
            count_arg(args, 0).map(|n| IntOut::Same(rotate(width, recv, n, false)))
        }
        BuiltinId::SwapBytes => Ok(IntOut::Same(from_raw(width, swap_bytes(width, recv)))),
        BuiltinId::ToLeBytes => Ok(IntOut::Bytes(to_bytes(width, recv, ByteOrder::Le))),
        BuiltinId::ToBeBytes => Ok(IntOut::Bytes(to_bytes(width, recv, ByteOrder::Be))),
        BuiltinId::ToNeBytes => Ok(IntOut::Bytes(to_bytes(width, recv, ByteOrder::Ne))),
        BuiltinId::ReverseBits => {
            let value = raw(width, recv).reverse_bits() >> (128 - bits);
            Ok(IntOut::Same(from_raw(width, value)))
        }
        _ => return None,
    };
    Some(out)
}

fn int_query_method(
    name: BuiltinId,
    width: IntWidth,
    recv: i128,
    args: &[i128],
) -> Option<Result<IntOut>> {
    if let Some(out) = int_range_family(name, width, recv, args)
        .or_else(|| int_bit_family(name, width, recv, args))
    {
        return Some(out);
    }
    let bits = width.bits();
    let out = match name {
        // From the real value, not the saturated image. serde answers by
        // range, so a negative number is not a u64.
        BuiltinId::AsI64 => Ok(IntOut::Checked(i64::try_from(recv).ok().map(i128::from))),
        BuiltinId::AsU64 => Ok(IntOut::Checked(u64::try_from(recv).ok().map(i128::from))),
        BuiltinId::AsF64 => Ok(IntOut::SomeFloat(AsPrimitive::<f64>::as_(recv))),
        BuiltinId::Min => arg(args, 0).map(|b| IntOut::Same(recv.min(b))),
        BuiltinId::Max => arg(args, 0).map(|b| IntOut::Same(recv.max(b))),
        BuiltinId::Clamp => arg(args, 0).and_then(|low| {
            let high = arg(args, 1)?;
            if low > high {
                bail!("min > max. min = {low}, max = {high}");
            }
            Ok(IntOut::Same(recv.clamp(low, high)))
        }),
        BuiltinId::Cmp => arg(args, 0).map(|b| IntOut::Ordering(recv.cmp(&b))),
        BuiltinId::IsMultipleOf => arg(args, 0).map(|b| {
            // Only zero is a multiple of zero, no panic. Taking the remainder
            // by zero crashed the interpreter itself.
            IntOut::Bool(if b == 0 { recv == 0 } else { recv % b == 0 })
        }),
        BuiltinId::DivEuclid => arg(args, 0).and_then(|b| {
            if b == 0 {
                bail!("attempt to divide by zero");
            }
            match in_range(width, recv.div_euclid(b)) {
                Some(value) => Ok(IntOut::Same(value)),
                None => bail!("attempt to divide with overflow"),
            }
        }),
        BuiltinId::RemEuclid => arg(args, 0).and_then(|b| {
            if b == 0 {
                bail!("attempt to calculate the remainder with a divisor of zero");
            }
            // `MIN % -1` overflows in real Rust even though the euclidean
            // remainder is 0. i128 hides that, so it is checked here.
            if width.is_signed() && b == -1 && recv == width.min() {
                bail!("attempt to calculate the remainder with overflow");
            }
            match in_range(width, recv.rem_euclid(b)) {
                Some(value) => Ok(IntOut::Same(value)),
                None => bail!("attempt to calculate the remainder with overflow"),
            }
        }),
        BuiltinId::Isqrt => {
            if recv < 0 {
                Err(anyhow::anyhow!(
                    "argument of integer square root cannot be negative"
                ))
            } else {
                Ok(IntOut::Same(isqrt(recv)))
            }
        }
        // Rounds towards zero like the i128 division.
        BuiltinId::Midpoint => arg(args, 0).map(|b| IntOut::Same(recv.midpoint(b))),
        BuiltinId::CheckedRemEuclid => arg(args, 0).map(|b| {
            IntOut::Checked(
                if b == 0 || (width.is_signed() && b == -1 && recv == width.min()) {
                    None
                } else {
                    in_range(width, recv.rem_euclid(b))
                },
            )
        }),
        BuiltinId::LeadingOnes => {
            let value = raw(width, recv) << (128 - bits);
            Ok(IntOut::Count(value.leading_ones()))
        }
        BuiltinId::TrailingOnes => {
            let value = raw(width, recv);
            Ok(IntOut::Count(value.trailing_ones().min(bits)))
        }
        _ => return None,
    };
    Some(out)
}

fn isqrt(value: i128) -> i128 {
    if value < 2 {
        return value;
    }
    let mut low = 1i128;
    let mut high = value.min(1i128 << 64);
    while low < high {
        let mid = (low + high + 1) / 2;
        if mid <= value / mid {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    low
}

fn rotate(width: IntWidth, value: i128, amount: u32, left: bool) -> i128 {
    let bits = width.bits();
    let shift = amount % bits;
    let bit_value = raw(width, value);
    if shift == 0 {
        return from_raw(width, bit_value);
    }
    let rotated = if left {
        (bit_value << shift) | (bit_value >> (bits - shift))
    } else {
        (bit_value >> shift) | (bit_value << (bits - shift))
    };
    from_raw(width, rotated)
}

fn swap_bytes(width: IntWidth, value: i128) -> u128 {
    let bytes = (width.bits() / 8) as usize;
    let source = raw(width, value);
    let mut out: u128 = 0;
    for index in 0..bytes {
        let byte = (source >> (index * 8)) & 0xff;
        out |= byte << ((bytes - 1 - index) * 8);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(name: BuiltinId, width: IntWidth, recv: i128, args: &[i128]) -> i128 {
        match int_method(name, width, recv, args).expect("known method") {
            Ok(IntOut::Same(value)) => value,
            Ok(_) => panic!("{name} did not answer a value"),
            Err(error) => panic!("{name} failed: {error}"),
        }
    }

    /// A `u64` past `i64::MAX` was once clamped before the method saw it.
    #[test]
    fn a_u64_past_i64_max_keeps_its_value() {
        let big = i128::from(u64::MAX);
        assert_eq!(same(BuiltinId::Max, IntWidth::U64, big, &[0]), big);
        assert_eq!(same(BuiltinId::Min, IntWidth::U64, big, &[big]), big);
        assert_eq!(
            same(BuiltinId::SaturatingAdd, IntWidth::U64, big, &[0]),
            big
        );
    }

    /// Saturation at the receiver's real bounds.
    #[test]
    fn saturating_uses_the_real_width() {
        assert_eq!(
            same(BuiltinId::SaturatingAdd, IntWidth::U8, 200, &[100]),
            255
        );
        assert_eq!(
            same(BuiltinId::SaturatingSub, IntWidth::I8, -100, &[100]),
            -128
        );
        assert_eq!(same(BuiltinId::SaturatingMul, IntWidth::U8, 5, &[100]), 255);
        assert_eq!(same(BuiltinId::SaturatingSub, IntWidth::U8, 5, &[100]), 0);
    }

    #[test]
    fn pow_and_abs_panic_where_debug_rust_panics() {
        let overflow = int_method(BuiltinId::Pow, IntWidth::U8, 16, &[2]).expect("known");
        assert!(overflow.is_err(), "16u8.pow(2) must overflow");
        assert_eq!(same(BuiltinId::Pow, IntWidth::U8, 15, &[2]), 225);

        let negate = int_method(BuiltinId::Abs, IntWidth::I8, -128, &[]).expect("known");
        assert!(negate.is_err(), "i8::MIN.abs() must overflow");
        assert_eq!(same(BuiltinId::Abs, IntWidth::I8, -127, &[]), 127);
    }

    /// A zero divisor once crashed the interpreter process.
    #[test]
    fn is_multiple_of_zero_answers_instead_of_crashing() {
        let answer = int_method(BuiltinId::IsMultipleOf, IntWidth::U64, 0, &[0]).expect("known");
        assert!(matches!(answer, Ok(IntOut::Bool(true))));
        let answer = int_method(BuiltinId::IsMultipleOf, IntWidth::U64, 5, &[0]).expect("known");
        assert!(matches!(answer, Ok(IntOut::Bool(false))));
    }

    #[test]
    fn wrapping_and_checked_follow_the_width() {
        assert_eq!(same(BuiltinId::WrappingAdd, IntWidth::U8, 250, &[10]), 4);
        assert_eq!(same(BuiltinId::WrappingSub, IntWidth::U8, 0, &[1]), 255);
        assert_eq!(same(BuiltinId::WrappingMul, IntWidth::I8, 100, &[3]), 44);
        let checked = int_method(BuiltinId::CheckedAdd, IntWidth::U8, 250, &[10]).expect("known");
        assert!(matches!(checked, Ok(IntOut::Checked(None))));
        let checked = int_method(BuiltinId::CheckedAdd, IntWidth::U8, 1, &[2]).expect("known");
        assert!(matches!(checked, Ok(IntOut::Checked(Some(3)))));
    }

    #[test]
    fn checked_shifts_gate_on_the_width() {
        let shifted = int_method(BuiltinId::CheckedShl, IntWidth::U8, 200, &[1]).expect("known");
        assert!(matches!(shifted, Ok(IntOut::Checked(Some(144)))));
        let shifted = int_method(BuiltinId::CheckedShl, IntWidth::U8, 1, &[8]).expect("known");
        assert!(matches!(shifted, Ok(IntOut::Checked(None))));
        let shifted = int_method(BuiltinId::CheckedShr, IntWidth::I8, -128, &[2]).expect("known");
        assert!(matches!(shifted, Ok(IntOut::Checked(Some(-32)))));
        let shifted = int_method(BuiltinId::CheckedShr, IntWidth::I8, -1, &[8]).expect("known");
        assert!(matches!(shifted, Ok(IntOut::Checked(None))));
    }

    #[test]
    fn bit_methods_use_the_width_not_the_storage() {
        let count = int_method(BuiltinId::CountOnes, IntWidth::U8, 250, &[]).expect("known");
        assert!(matches!(count, Ok(IntOut::Count(6))));
        let count = int_method(BuiltinId::LeadingZeros, IntWidth::U8, 1, &[]).expect("known");
        assert!(matches!(count, Ok(IntOut::Count(7))));
        let count = int_method(BuiltinId::TrailingZeros, IntWidth::U8, 0, &[]).expect("known");
        assert!(matches!(count, Ok(IntOut::Count(8))));
        assert_eq!(
            same(BuiltinId::SwapBytes, IntWidth::U16, 0x1234, &[]),
            0x3412
        );
        assert_eq!(
            same(BuiltinId::ReverseBits, IntWidth::U8, 0b1000_0000, &[]),
            1
        );
        assert_eq!(
            same(BuiltinId::RotateLeft, IntWidth::U8, 0b1000_0001, &[1]),
            0b11
        );
    }

    fn bytes(name: BuiltinId, width: IntWidth, recv: i128) -> Vec<u8> {
        match int_method(name, width, recv, &[]).expect("known method") {
            Ok(IntOut::Bytes(out)) => out,
            Ok(_) => panic!("{name} did not answer bytes"),
            Err(error) => panic!("{name} failed: {error}"),
        }
    }

    /// The 2 orders must disagree, or an endianness bug reads as correct.
    #[test]
    fn byte_conversions_keep_their_order() {
        assert_eq!(
            bytes(BuiltinId::ToLeBytes, IntWidth::U32, 0x1234_5678),
            [0x78, 0x56, 0x34, 0x12]
        );
        assert_eq!(
            bytes(BuiltinId::ToBeBytes, IntWidth::U32, 0x1234_5678),
            [0x12, 0x34, 0x56, 0x78]
        );
        assert_eq!(bytes(BuiltinId::ToLeBytes, IntWidth::U8, 0xab), [0xab]);
        assert_eq!(
            bytes(BuiltinId::ToBeBytes, IntWidth::U64, 1),
            [0, 0, 0, 0, 0, 0, 0, 1]
        );
        let le = from_bytes(IntWidth::U32, ByteOrder::Le, &[0x78, 0x56, 0x34, 0x12]).unwrap();
        let be = from_bytes(IntWidth::U32, ByteOrder::Be, &[0x78, 0x56, 0x34, 0x12]).unwrap();
        assert_eq!(le, 0x1234_5678);
        assert_eq!(be, 0x7856_3412);
    }

    /// An unsigned width reads the same bytes as a positive number.
    #[test]
    fn byte_conversions_respect_the_sign() {
        assert_eq!(bytes(BuiltinId::ToBeBytes, IntWidth::I16, -2), [0xff, 0xfe]);
        assert_eq!(bytes(BuiltinId::ToLeBytes, IntWidth::I16, -2), [0xfe, 0xff]);
        let signed = from_bytes(IntWidth::I32, ByteOrder::Le, &[0xff, 0xff, 0xff, 0xff]).unwrap();
        let unsigned = from_bytes(IntWidth::U32, ByteOrder::Le, &[0xff, 0xff, 0xff, 0xff]).unwrap();
        assert_eq!(signed, -1);
        assert_eq!(unsigned, 0xffff_ffff);
        let low = from_bytes(IntWidth::I8, ByteOrder::Be, &[0x80]).unwrap();
        assert_eq!(low, -128);
    }

    #[test]
    fn from_bytes_rejects_a_shape_the_type_checker_would_have() {
        assert!(from_bytes(IntWidth::U32, ByteOrder::Le, &[1, 2, 3]).is_err());
        assert!(from_bytes(IntWidth::U16, ByteOrder::Le, &[1, 256]).is_err());
        assert!(from_bytes(IntWidth::U16, ByteOrder::Le, &[1, -1]).is_err());
    }

    #[test]
    fn unknown_names_fall_through() {
        assert!(int_method(BuiltinId::Sqrt, IntWidth::I64, 4, &[]).is_none());
        assert!(int_method(BuiltinId::Abs, IntWidth::U8, 4, &[]).is_none());
        assert!(int_method(BuiltinId::Signum, IntWidth::U8, 4, &[]).is_none());
    }
}
