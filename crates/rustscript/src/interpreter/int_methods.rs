//! Width aware integer methods. The receiver arrives here with its true value and width, so
//! `200u8.saturating_add(100)` is 255 and a `u64` past `i64::MAX` is not clamped.

use num_traits::AsPrimitive;
use std::cmp::Ordering;

use anyhow::{Result, bail};

use super::bytecode::BuiltinId;
use super::numeric::IntWidth;

pub enum IntOut {
    Same(i128),
    /// a bit count, always `u32`
    Count(u32),
    Bool(bool),
    /// `checked_*`, and the serde `as_i64` and `as_u64`, a value only when it fits
    Checked(Option<i128>),
    /// `as_f64`, always a `Some` in serde
    SomeFloat(f64),
    Ordering(Ordering),
    /// `to_le_bytes` and its siblings
    Bytes(Vec<u8>),
    /// `overflowing_*`
    Overflowing(i128, bool),
    /// `checked_ilog2`
    CheckedCount(Option<u32>),
    /// `abs_diff`, the unsigned type of the receiver's size
    Unsigned(i128),
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

/// Real Rust takes an exact `[u8; N]`, so a wrong length is an error and not a guess.
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

/// Over the receiver's real width, so a signed value writes two's complement.
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

/// Sign extends when the width is signed. What `wrapping_*` and the bit methods return through.
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

/// Checked step by step so the panic lands where debug Rust panics. The multiply is checked in
/// i128 too, a `u64` product can pass what it holds.
fn pow(width: IntWidth, base: i128, exponent: u32) -> Result<i128> {
    let mut result: i128 = 1;
    for _ in 0..exponent {
        let Some(next) = result.checked_mul(base) else {
            bail!("attempt to exponentiate with overflow");
        };
        result = next;
        if result < width.min() || result > width.max() {
            bail!("attempt to exponentiate with overflow");
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

/// Methods whose argument is a `u32` amount, so the dispatch must not unify the receiver width with it.
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
        BuiltinId::AbsDiff => arg(args, 0).map(|b| IntOut::Unsigned((recv - b).abs())),
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
            // `MIN % -1` overflows in the receiver width even though the i128 remainder is 0
            IntOut::Checked(
                if b == 0 || (width.is_signed() && b == -1 && recv == width.min()) {
                    None
                } else {
                    in_range(width, recv % b)
                },
            )
        }),
        // a shift is checked on the amount alone
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

/// Some of these exist on 1 signedness only.
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

/// All over the raw bits of the receiver width.
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
        // from the real value, not the saturated image, serde checks by range so a negative
        // number is not a u64
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
            // only zero is a multiple of zero, no panic. Taking the remainder by zero would crash
            // the interpreter itself
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
            // `MIN % -1` overflows in real Rust even though the euclidean remainder is 0, i128
            // hides that so check here
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
        // rounds towards zero like the i128 division
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

mod big;

pub use big::big_int_method;

#[cfg(test)]
mod tests;
