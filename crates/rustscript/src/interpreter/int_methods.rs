//! Width-aware integer methods, one table for the whole dispatch.
//!
//! These used to run on the i64 image that `bridge_image` produces, which lost
//! two things. The width, so `200u8.saturating_add(100)` saturated at
//! `i64::MAX` and answered 300 where real Rust answers 255. And the range, so
//! every method on a `u64` past `i64::MAX` saw `i64::MAX` instead of the real
//! value, which made `big.max(0)` answer `9223372036854775807`.
//!
//! So the receiver arrives here as its true value and its true width, and each
//! method computes in that width and panics exactly where debug Rust panics.

use num_traits::AsPrimitive;
use std::cmp::Ordering;

use anyhow::{Result, bail};

use super::bytecode::{BuiltinId, BuiltinId as B};
use super::numeric::IntWidth;

/// What an integer method produced, materialized into a value by the caller.
pub enum IntOut {
    /// A value in the receiver's own width.
    Same(i128),
    /// A bit count, always `u32` in real Rust.
    Count(u32),
    Bool(bool),
    /// `checked_*`, `Some` in the receiver's width or `None` on overflow.
    /// The serde `as_i64` and `as_u64` answer through this too, they are the
    /// same shape, a value only when it fits.
    Checked(Option<i128>),
    /// `as_f64`, which is always a `Some` in serde.
    SomeFloat(f64),
    Ordering(Ordering),
    /// `to_le_bytes` and its siblings, one byte per byte of the width.
    Bytes(Vec<u8>),
}

/// Byte order of an integer byte conversion. `Ne` is the target's own order,
/// read from the host the interpreter runs on, so a script answers what
/// compiled Rust on the same machine answers.
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

/// The order a `T::from_le_bytes` style associated function names, or `None`
/// when the name is not one of the three byte conversions.
pub fn from_bytes_order(name: &str) -> Option<ByteOrder> {
    Some(match name {
        "from_le_bytes" => ByteOrder::Le,
        "from_be_bytes" => ByteOrder::Be,
        "from_ne_bytes" => ByteOrder::Ne,
        _ => return None,
    })
}

/// `T::from_le_bytes` and its siblings. Real Rust takes an exact `[u8; N]`, so
/// a wrong length or an element outside a byte only reaches here from a script
/// that never passed the check gate, and it is an error rather than a guess.
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

/// `x.to_le_bytes()` and its siblings, over the receiver's real width, so a
/// signed value writes its two's complement bytes.
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

/// Raw bits of a value in its width, for the bit twiddling methods. A
/// 128-bit value already stores its raw bits, and its mask would not fit.
fn raw(width: IntWidth, value: i128) -> u128 {
    let bits = width.bits();
    if bits == 128 {
        return value.cast_unsigned();
    }
    let mask = (1u128 << bits) - 1;
    AsPrimitive::<u128>::as_(value) & mask
}

/// Reinterpret raw bits back as a value of the width, sign extending when the
/// width is signed. This is what `wrapping_*` and the bit methods return
/// through.
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

/// `pow`, checked step by step so the panic lands where debug Rust's does.
/// The multiply is checked in i128 too, since a `u64` receiver can carry the
/// product past what an i128 holds.
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

/// A shift or rotate amount, which is a `u32` in real Rust.
fn count_arg(args: &[i128], index: usize) -> Result<u32> {
    let value = arg(args, index)?;
    match u32::try_from(value) {
        Ok(count) => Ok(count),
        Err(_) => bail!("shift amount does not fit u32"),
    }
}

/// Answer an integer method in its real width, or `None` when the name is not
/// one of these so the caller falls through to its own dispatch.
/// Methods whose argument is a `u32` amount of its own rather than a value of
/// the receiver's type, so the dispatch must not unify the receiver's width
/// with the argument's.
pub fn takes_amount_arg(name: BuiltinId) -> bool {
    matches!(
        name,
        B::Pow
            | B::Powi
            | B::CheckedPow
            | B::RotateLeft
            | B::RotateRight
            | B::CheckedShl
            | B::CheckedShr
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

/// Stamps the shared native method body for one 128-bit type. `$decode`
/// turns raw storage bits into the type, `$encode` a result back into bits.
/// The names here mirror the i128 pipeline above; new names must go into
/// both, and the coverage harvest reads the pipeline's two halves.
macro_rules! big_methods {
    ($fn_name:ident, $ty:ty, $decode:expr, $encode:expr) => {
        fn $fn_name(name: BuiltinId, recv_bits: i128, args: &[i128]) -> Option<Result<IntOut>> {
            let decode = $decode;
            let encode = $encode;
            let recv: $ty = decode(recv_bits);
            let val = |i: usize| -> Result<$ty> { Ok(decode(arg(args, i)?)) };
            let same = |v: $ty| IntOut::Same(encode(v));
            let checked = |v: Option<$ty>| IntOut::Checked(v.map(encode));
            let out: Result<IntOut> = match name {
                B::SaturatingAdd => val(0).map(|b| same(recv.saturating_add(b))),
                B::SaturatingSub => val(0).map(|b| same(recv.saturating_sub(b))),
                B::SaturatingMul => val(0).map(|b| same(recv.saturating_mul(b))),
                B::WrappingAdd => val(0).map(|b| same(recv.wrapping_add(b))),
                B::WrappingSub => val(0).map(|b| same(recv.wrapping_sub(b))),
                B::WrappingMul => val(0).map(|b| same(recv.wrapping_mul(b))),
                B::WrappingNeg => Ok(same(recv.wrapping_neg())),
                B::CheckedAdd => val(0).map(|b| checked(recv.checked_add(b))),
                B::CheckedSub => val(0).map(|b| checked(recv.checked_sub(b))),
                B::CheckedMul => val(0).map(|b| checked(recv.checked_mul(b))),
                B::CheckedNeg => Ok(checked(recv.checked_neg())),
                B::CheckedDiv => val(0).map(|b| checked(recv.checked_div(b))),
                B::CheckedRem => val(0).map(|b| checked(recv.checked_rem(b))),
                B::CheckedShl => count_arg(args, 0).map(|n| checked(recv.checked_shl(n))),
                B::CheckedShr => count_arg(args, 0).map(|n| checked(recv.checked_shr(n))),
                B::Pow => count_arg(args, 0).and_then(|e| match recv.checked_pow(e) {
                    Some(v) => Ok(same(v)),
                    None => bail!("attempt to multiply with overflow"),
                }),
                B::CheckedPow => count_arg(args, 0).map(|e| checked(recv.checked_pow(e))),
                B::DivEuclid => val(0).and_then(|b| {
                    if b == 0 {
                        bail!("attempt to divide by zero");
                    }
                    match recv.checked_div_euclid(b) {
                        Some(v) => Ok(same(v)),
                        None => bail!("attempt to divide with overflow"),
                    }
                }),
                B::RemEuclid => val(0).and_then(|b| {
                    if b == 0 {
                        bail!("attempt to calculate the remainder with a divisor of zero");
                    }
                    match recv.checked_rem_euclid(b) {
                        Some(v) => Ok(same(v)),
                        None => bail!("attempt to calculate the remainder with overflow"),
                    }
                }),
                B::Min => val(0).map(|b| same(recv.min(b))),
                B::Max => val(0).map(|b| same(recv.max(b))),
                B::Clamp => val(0).and_then(|low| {
                    let high = val(1)?;
                    if low > high {
                        bail!("min > max. min = {low}, max = {high}");
                    }
                    Ok(same(recv.clamp(low, high)))
                }),
                B::Cmp => val(0).map(|b| IntOut::Ordering(recv.cmp(&b))),
                B::IsMultipleOf => val(0).map(|b| {
                    IntOut::Bool(match b {
                        0 => recv == 0,
                        // The only `None` remainder is MIN % -1, which is 0.
                        _ => recv.checked_rem(b).is_none_or(|r| r == 0),
                    })
                }),
                B::CountOnes => Ok(IntOut::Count(recv.count_ones())),
                B::CountZeros => Ok(IntOut::Count(recv.count_zeros())),
                B::LeadingZeros => Ok(IntOut::Count(recv.leading_zeros())),
                B::TrailingZeros => Ok(IntOut::Count(recv.trailing_zeros())),
                B::RotateLeft => count_arg(args, 0).map(|n| same(recv.rotate_left(n))),
                B::RotateRight => count_arg(args, 0).map(|n| same(recv.rotate_right(n))),
                B::SwapBytes => Ok(same(recv.swap_bytes())),
                B::ReverseBits => Ok(same(recv.reverse_bits())),
                B::ToLeBytes => Ok(IntOut::Bytes(recv.to_le_bytes().to_vec())),
                B::ToBeBytes => Ok(IntOut::Bytes(recv.to_be_bytes().to_vec())),
                B::ToNeBytes => Ok(IntOut::Bytes(recv.to_ne_bytes().to_vec())),
                B::AsI64 => Ok(IntOut::Checked(i64::try_from(recv).ok().map(i128::from))),
                B::AsU64 => Ok(IntOut::Checked(u64::try_from(recv).ok().map(i128::from))),
                B::AsF64 => Ok(IntOut::SomeFloat(AsPrimitive::<f64>::as_(recv))),
                _ => return None,
            };
            Some(out)
        }
    };
}

big_methods!(
    u128_method,
    u128,
    |bits: i128| bits.cast_unsigned(),
    |v: u128| { v.cast_signed() }
);
big_methods!(i128_method, i128, |bits: i128| bits, |v: i128| v);

/// Native method cores for the 128-bit receivers. The i128 pipeline above
/// cannot host them: a `u128` past `i128::MAX` stores as negative bits and
/// its bounds do not fit an i128. `recv` and every `Same` or `Checked`
/// payload are raw storage bits, u128 reinterpreted, exactly what
/// `Value::Big` carries. The signed-only names live here because the
/// stamped body must compile for u128 too.
pub fn big_int_method(
    name: BuiltinId,
    width: IntWidth,
    recv: i128,
    args: &[i128],
) -> Option<Result<IntOut>> {
    match width {
        IntWidth::U128 => match name {
            B::Isqrt => Some(Ok(IntOut::Same(recv.cast_unsigned().isqrt().cast_signed()))),
            _ => u128_method(name, recv, args),
        },
        IntWidth::I128 => match name {
            B::Isqrt => Some(if recv < 0 {
                Err(anyhow::anyhow!(
                    "argument of integer square root cannot be negative"
                ))
            } else {
                Ok(IntOut::Same(recv.isqrt()))
            }),
            B::Abs => Some(if recv == i128::MIN {
                Err(anyhow::anyhow!("attempt to negate with overflow"))
            } else {
                Ok(IntOut::Same(recv.abs()))
            }),
            B::Signum => Some(Ok(IntOut::Same(recv.signum()))),
            _ => i128_method(name, recv, args),
        },
        _ => None,
    }
}

/// The arithmetic families: saturating, wrapping, checked, pow, abs, signum.
fn int_arith_method(
    name: BuiltinId,
    width: IntWidth,
    recv: i128,
    args: &[i128],
) -> Option<Result<IntOut>> {
    let bits = width.bits();
    let out = match name {
        B::SaturatingAdd => {
            arg(args, 0).map(|b| IntOut::Same(saturate(width, recv.saturating_add(b))))
        }
        B::SaturatingSub => {
            arg(args, 0).map(|b| IntOut::Same(saturate(width, recv.saturating_sub(b))))
        }
        B::SaturatingMul => {
            arg(args, 0).map(|b| IntOut::Same(saturate(width, recv.saturating_mul(b))))
        }
        B::WrappingAdd => arg(args, 0).map(|b| {
            IntOut::Same(from_raw(
                width,
                AsPrimitive::<u128>::as_(recv.wrapping_add(b)),
            ))
        }),
        B::WrappingSub => arg(args, 0).map(|b| {
            IntOut::Same(from_raw(
                width,
                AsPrimitive::<u128>::as_(recv.wrapping_sub(b)),
            ))
        }),
        B::WrappingMul => arg(args, 0).map(|b| {
            IntOut::Same(from_raw(
                width,
                AsPrimitive::<u128>::as_(recv.wrapping_mul(b)),
            ))
        }),
        B::WrappingNeg => Ok(IntOut::Same(from_raw(
            width,
            AsPrimitive::<u128>::as_(-recv),
        ))),
        B::CheckedAdd => arg(args, 0)
            .map(|b| IntOut::Checked(recv.checked_add(b).and_then(|v| in_range(width, v)))),
        B::CheckedSub => arg(args, 0)
            .map(|b| IntOut::Checked(recv.checked_sub(b).and_then(|v| in_range(width, v)))),
        B::CheckedMul => arg(args, 0)
            .map(|b| IntOut::Checked(recv.checked_mul(b).and_then(|v| in_range(width, v)))),
        B::CheckedNeg => Ok(IntOut::Checked(in_range(width, -recv))),
        B::CheckedDiv => arg(args, 0).map(|b| {
            IntOut::Checked(if b == 0 {
                None
            } else {
                in_range(width, recv / b)
            })
        }),
        B::CheckedRem => arg(args, 0).map(|b| {
            // MIN % -1 overflows in the receiver's width even though the
            // i128 remainder is 0, so real Rust answers None for it.
            IntOut::Checked(
                if b == 0 || (width.is_signed() && b == -1 && recv == width.min()) {
                    None
                } else {
                    in_range(width, recv % b)
                },
            )
        }),
        // A shift is checked on the amount alone, `None` at the width and
        // beyond, and bits shifted past the width are simply dropped.
        B::CheckedShl => count_arg(args, 0)
            .map(|n| IntOut::Checked((n < bits).then(|| from_raw(width, raw(width, recv) << n)))),
        B::CheckedShr => count_arg(args, 0).map(|n| {
            IntOut::Checked((n < bits).then(|| {
                if width.is_signed() {
                    recv >> n
                } else {
                    from_raw(width, raw(width, recv) >> n)
                }
            }))
        }),
        B::Pow => count_arg(args, 0).and_then(|e| pow(width, recv, e).map(IntOut::Same)),
        B::CheckedPow => count_arg(args, 0).map(|e| IntOut::Checked(pow(width, recv, e).ok())),
        B::Abs => {
            if !width.is_signed() {
                return None;
            }
            if recv == width.min() {
                Err(anyhow::anyhow!("attempt to negate with overflow"))
            } else {
                Ok(IntOut::Same(recv.abs()))
            }
        }
        B::Signum => {
            if !width.is_signed() {
                return None;
            }
            Ok(IntOut::Same(recv.signum()))
        }
        _ => return None,
    };
    Some(out)
}

/// Accessors, comparisons, euclid forms, and the bit and byte views.
fn int_query_method(
    name: BuiltinId,
    width: IntWidth,
    recv: i128,
    args: &[i128],
) -> Option<Result<IntOut>> {
    let bits = width.bits();
    let out = match name {
        // The serde_json integer accessors, answered from the real value
        // rather than the saturated i64 image. serde answers these by range,
        // so a negative number is not a u64 and one past `i64::MAX` is not an
        // i64. The saturated image made both of those answer the wrong thing.
        B::AsI64 => Ok(IntOut::Checked(i64::try_from(recv).ok().map(i128::from))),
        B::AsU64 => Ok(IntOut::Checked(u64::try_from(recv).ok().map(i128::from))),
        B::AsF64 => Ok(IntOut::SomeFloat(AsPrimitive::<f64>::as_(recv))),
        B::Min => arg(args, 0).map(|b| IntOut::Same(recv.min(b))),
        B::Max => arg(args, 0).map(|b| IntOut::Same(recv.max(b))),
        B::Clamp => arg(args, 0).and_then(|low| {
            let high = arg(args, 1)?;
            if low > high {
                bail!("min > max. min = {low}, max = {high}");
            }
            Ok(IntOut::Same(recv.clamp(low, high)))
        }),
        B::Cmp => arg(args, 0).map(|b| IntOut::Ordering(recv.cmp(&b))),
        B::IsMultipleOf => arg(args, 0).map(|b| {
            // Real Rust defines a zero divisor as "only zero is a multiple of
            // zero" rather than a panic, so the remainder is never taken by
            // zero here. Taking it crashed the interpreter itself.
            IntOut::Bool(if b == 0 { recv == 0 } else { recv % b == 0 })
        }),
        B::DivEuclid => arg(args, 0).and_then(|b| {
            if b == 0 {
                bail!("attempt to divide by zero");
            }
            match in_range(width, recv.div_euclid(b)) {
                Some(value) => Ok(IntOut::Same(value)),
                None => bail!("attempt to divide with overflow"),
            }
        }),
        B::RemEuclid => arg(args, 0).and_then(|b| {
            if b == 0 {
                bail!("attempt to calculate the remainder with a divisor of zero");
            }
            // Real Rust takes `self % rhs` first, and MIN % -1 overflows
            // there even though the euclidean remainder itself would be 0.
            // Computing in i128 hides that overflow, so it is checked here.
            if width.is_signed() && b == -1 && recv == width.min() {
                bail!("attempt to calculate the remainder with overflow");
            }
            match in_range(width, recv.rem_euclid(b)) {
                Some(value) => Ok(IntOut::Same(value)),
                None => bail!("attempt to calculate the remainder with overflow"),
            }
        }),
        B::Isqrt => {
            if recv < 0 {
                Err(anyhow::anyhow!(
                    "argument of integer square root cannot be negative"
                ))
            } else {
                Ok(IntOut::Same(isqrt(recv)))
            }
        }
        B::CountOnes => Ok(IntOut::Count(raw(width, recv).count_ones())),
        B::CountZeros => Ok(IntOut::Count(bits - raw(width, recv).count_ones())),
        B::LeadingZeros => {
            let value = raw(width, recv);
            Ok(IntOut::Count(if value == 0 {
                bits
            } else {
                value.leading_zeros() - (128 - bits)
            }))
        }
        B::TrailingZeros => {
            let value = raw(width, recv);
            Ok(IntOut::Count(if value == 0 {
                bits
            } else {
                value.trailing_zeros()
            }))
        }
        B::RotateLeft => count_arg(args, 0).map(|n| IntOut::Same(rotate(width, recv, n, true))),
        B::RotateRight => count_arg(args, 0).map(|n| IntOut::Same(rotate(width, recv, n, false))),
        B::SwapBytes => Ok(IntOut::Same(from_raw(width, swap_bytes(width, recv)))),
        B::ToLeBytes => Ok(IntOut::Bytes(to_bytes(width, recv, ByteOrder::Le))),
        B::ToBeBytes => Ok(IntOut::Bytes(to_bytes(width, recv, ByteOrder::Be))),
        B::ToNeBytes => Ok(IntOut::Bytes(to_bytes(width, recv, ByteOrder::Ne))),
        B::ReverseBits => {
            let value = raw(width, recv).reverse_bits() >> (128 - bits);
            Ok(IntOut::Same(from_raw(width, value)))
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

    /// The regression the differential generator found: the whole numeric
    /// surface ran on an i64 image, so a `u64` past `i64::MAX` was clamped
    /// before the method ever saw it.
    #[test]
    fn a_u64_past_i64_max_keeps_its_value() {
        let big = i128::from(u64::MAX);
        assert_eq!(same(B::Max, IntWidth::U64, big, &[0]), big);
        assert_eq!(same(B::Min, IntWidth::U64, big, &[big]), big);
        assert_eq!(same(B::SaturatingAdd, IntWidth::U64, big, &[0]), big);
    }

    /// Saturation happens at the receiver's real bounds, not at i64's.
    #[test]
    fn saturating_uses_the_real_width() {
        assert_eq!(same(B::SaturatingAdd, IntWidth::U8, 200, &[100]), 255);
        assert_eq!(same(B::SaturatingSub, IntWidth::I8, -100, &[100]), -128);
        assert_eq!(same(B::SaturatingMul, IntWidth::U8, 5, &[100]), 255);
        assert_eq!(same(B::SaturatingSub, IntWidth::U8, 5, &[100]), 0);
    }

    #[test]
    fn pow_and_abs_panic_where_debug_rust_panics() {
        let overflow = int_method(B::Pow, IntWidth::U8, 16, &[2]).expect("known");
        assert!(overflow.is_err(), "16u8.pow(2) must overflow");
        assert_eq!(same(B::Pow, IntWidth::U8, 15, &[2]), 225);

        let negate = int_method(B::Abs, IntWidth::I8, -128, &[]).expect("known");
        assert!(negate.is_err(), "i8::MIN.abs() must overflow");
        assert_eq!(same(B::Abs, IntWidth::I8, -127, &[]), 127);
    }

    /// A zero divisor here once took the remainder anyway and crashed the
    /// interpreter process with its own host panic.
    #[test]
    fn is_multiple_of_zero_answers_instead_of_crashing() {
        let answer = int_method(B::IsMultipleOf, IntWidth::U64, 0, &[0]).expect("known");
        assert!(matches!(answer, Ok(IntOut::Bool(true))));
        let answer = int_method(B::IsMultipleOf, IntWidth::U64, 5, &[0]).expect("known");
        assert!(matches!(answer, Ok(IntOut::Bool(false))));
    }

    #[test]
    fn wrapping_and_checked_follow_the_width() {
        assert_eq!(same(B::WrappingAdd, IntWidth::U8, 250, &[10]), 4);
        assert_eq!(same(B::WrappingSub, IntWidth::U8, 0, &[1]), 255);
        assert_eq!(same(B::WrappingMul, IntWidth::I8, 100, &[3]), 44);
        let checked = int_method(B::CheckedAdd, IntWidth::U8, 250, &[10]).expect("known");
        assert!(matches!(checked, Ok(IntOut::Checked(None))));
        let checked = int_method(B::CheckedAdd, IntWidth::U8, 1, &[2]).expect("known");
        assert!(matches!(checked, Ok(IntOut::Checked(Some(3)))));
    }

    #[test]
    fn checked_shifts_gate_on_the_width() {
        let shifted = int_method(B::CheckedShl, IntWidth::U8, 200, &[1]).expect("known");
        assert!(matches!(shifted, Ok(IntOut::Checked(Some(144)))));
        let shifted = int_method(B::CheckedShl, IntWidth::U8, 1, &[8]).expect("known");
        assert!(matches!(shifted, Ok(IntOut::Checked(None))));
        let shifted = int_method(B::CheckedShr, IntWidth::I8, -128, &[2]).expect("known");
        assert!(matches!(shifted, Ok(IntOut::Checked(Some(-32)))));
        let shifted = int_method(B::CheckedShr, IntWidth::I8, -1, &[8]).expect("known");
        assert!(matches!(shifted, Ok(IntOut::Checked(None))));
    }

    #[test]
    fn bit_methods_use_the_width_not_the_storage() {
        let count = int_method(B::CountOnes, IntWidth::U8, 250, &[]).expect("known");
        assert!(matches!(count, Ok(IntOut::Count(6))));
        let count = int_method(B::LeadingZeros, IntWidth::U8, 1, &[]).expect("known");
        assert!(matches!(count, Ok(IntOut::Count(7))));
        let count = int_method(B::TrailingZeros, IntWidth::U8, 0, &[]).expect("known");
        assert!(matches!(count, Ok(IntOut::Count(8))));
        assert_eq!(same(B::SwapBytes, IntWidth::U16, 0x1234, &[]), 0x3412);
        assert_eq!(same(B::ReverseBits, IntWidth::U8, 0b1000_0000, &[]), 1);
        assert_eq!(same(B::RotateLeft, IntWidth::U8, 0b1000_0001, &[1]), 0b11);
    }

    fn bytes(name: BuiltinId, width: IntWidth, recv: i128) -> Vec<u8> {
        match int_method(name, width, recv, &[]).expect("known method") {
            Ok(IntOut::Bytes(out)) => out,
            Ok(_) => panic!("{name} did not answer bytes"),
            Err(error) => panic!("{name} failed: {error}"),
        }
    }

    /// The two orders must disagree, or an endianness bug reads as correct.
    #[test]
    fn byte_conversions_keep_their_order() {
        assert_eq!(
            bytes(B::ToLeBytes, IntWidth::U32, 0x1234_5678),
            [0x78, 0x56, 0x34, 0x12]
        );
        assert_eq!(
            bytes(B::ToBeBytes, IntWidth::U32, 0x1234_5678),
            [0x12, 0x34, 0x56, 0x78]
        );
        assert_eq!(bytes(B::ToLeBytes, IntWidth::U8, 0xab), [0xab]);
        assert_eq!(
            bytes(B::ToBeBytes, IntWidth::U64, 1),
            [0, 0, 0, 0, 0, 0, 0, 1]
        );
        let le = from_bytes(IntWidth::U32, ByteOrder::Le, &[0x78, 0x56, 0x34, 0x12]).unwrap();
        let be = from_bytes(IntWidth::U32, ByteOrder::Be, &[0x78, 0x56, 0x34, 0x12]).unwrap();
        assert_eq!(le, 0x1234_5678);
        assert_eq!(be, 0x7856_3412);
    }

    /// A signed width writes and reads two's complement, an unsigned one of the
    /// same size reads the very same bytes as a positive number.
    #[test]
    fn byte_conversions_respect_the_sign() {
        assert_eq!(bytes(B::ToBeBytes, IntWidth::I16, -2), [0xff, 0xfe]);
        assert_eq!(bytes(B::ToLeBytes, IntWidth::I16, -2), [0xfe, 0xff]);
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
        assert!(int_method(B::Sqrt, IntWidth::I64, 4, &[]).is_none());
        assert!(int_method(B::Abs, IntWidth::U8, 4, &[]).is_none());
        assert!(int_method(B::Signum, IntWidth::U8, 4, &[]).is_none());
    }
}
