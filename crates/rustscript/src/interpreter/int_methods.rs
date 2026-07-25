//! Width-aware integer methods, written once for both engines.
//!
//! These used to run on the i64 image that `bridge_image` produces, which lost
//! two things. The width, so `200u8.saturating_add(100)` saturated at
//! `i64::MAX` and answered 300 where real Rust answers 255. And the range, so
//! every method on a `u64` past `i64::MAX` saw `i64::MAX` instead of the real
//! value, which made `big.max(0)` answer `9223372036854775807`.
//!
//! So the receiver arrives here as its true value and its true width, and each
//! method computes in that width and panics exactly where debug Rust panics.

use std::cmp::Ordering;

use anyhow::{Result, bail};

use super::numeric::IntWidth;

/// What an integer method produced. Each engine materializes it into its own
/// value type.
pub enum IntOut {
    /// A value in the receiver's own width.
    Same(i128),
    /// A bit count, always `u32` in real Rust.
    Count(u32),
    Bool(bool),
    /// `checked_*`, `Some` in the receiver's width or `None` on overflow.
    Checked(Option<i128>),
    Ordering(Ordering),
}

/// Raw bits of a value in its width, for the bit twiddling methods.
fn raw(width: IntWidth, value: i128) -> u128 {
    let bits = width.bits();
    let mask = (1u128 << bits) - 1;
    (value as u128) & mask
}

/// Reinterpret raw bits back as a value of the width, sign extending when the
/// width is signed. This is what `wrapping_*` and the bit methods return
/// through.
fn from_raw(width: IntWidth, bits_value: u128) -> i128 {
    let bits = width.bits();
    let mask = (1u128 << bits) - 1;
    let truncated = bits_value & mask;
    if width.is_signed() && truncated >> (bits - 1) & 1 == 1 {
        truncated as i128 - (1i128 << bits)
    } else {
        truncated as i128
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
pub fn int_method(
    name: &str,
    width: IntWidth,
    recv: i128,
    args: &[i128],
) -> Option<Result<IntOut>> {
    let bits = width.bits();
    let out = match name {
        "saturating_add" => {
            arg(args, 0).map(|b| IntOut::Same(saturate(width, recv.saturating_add(b))))
        }
        "saturating_sub" => {
            arg(args, 0).map(|b| IntOut::Same(saturate(width, recv.saturating_sub(b))))
        }
        "saturating_mul" => {
            arg(args, 0).map(|b| IntOut::Same(saturate(width, recv.saturating_mul(b))))
        }
        "wrapping_add" => {
            arg(args, 0).map(|b| IntOut::Same(from_raw(width, recv.wrapping_add(b) as u128)))
        }
        "wrapping_sub" => {
            arg(args, 0).map(|b| IntOut::Same(from_raw(width, recv.wrapping_sub(b) as u128)))
        }
        "wrapping_mul" => {
            arg(args, 0).map(|b| IntOut::Same(from_raw(width, recv.wrapping_mul(b) as u128)))
        }
        "wrapping_neg" => Ok(IntOut::Same(from_raw(width, (-recv) as u128))),
        "checked_add" => arg(args, 0)
            .map(|b| IntOut::Checked(recv.checked_add(b).and_then(|v| in_range(width, v)))),
        "checked_sub" => arg(args, 0)
            .map(|b| IntOut::Checked(recv.checked_sub(b).and_then(|v| in_range(width, v)))),
        "checked_mul" => arg(args, 0)
            .map(|b| IntOut::Checked(recv.checked_mul(b).and_then(|v| in_range(width, v)))),
        "checked_neg" => Ok(IntOut::Checked(in_range(width, -recv))),
        "checked_div" => arg(args, 0).map(|b| {
            IntOut::Checked(if b == 0 {
                None
            } else {
                in_range(width, recv / b)
            })
        }),
        "checked_rem" => arg(args, 0).map(|b| {
            IntOut::Checked(if b == 0 {
                None
            } else {
                in_range(width, recv % b)
            })
        }),
        "pow" => count_arg(args, 0).and_then(|e| pow(width, recv, e).map(IntOut::Same)),
        "abs" => {
            if !width.is_signed() {
                return None;
            }
            if recv == width.min() {
                Err(anyhow::anyhow!("attempt to negate with overflow"))
            } else {
                Ok(IntOut::Same(recv.abs()))
            }
        }
        "signum" => {
            if !width.is_signed() {
                return None;
            }
            Ok(IntOut::Same(recv.signum()))
        }
        "min" => arg(args, 0).map(|b| IntOut::Same(recv.min(b))),
        "max" => arg(args, 0).map(|b| IntOut::Same(recv.max(b))),
        "clamp" => arg(args, 0).and_then(|low| {
            let high = arg(args, 1)?;
            if low > high {
                bail!("min > max. min = {low}, max = {high}");
            }
            Ok(IntOut::Same(recv.clamp(low, high)))
        }),
        "cmp" => arg(args, 0).map(|b| IntOut::Ordering(recv.cmp(&b))),
        "is_multiple_of" => arg(args, 0).map(|b| {
            // Real Rust defines a zero divisor as "only zero is a multiple of
            // zero" rather than a panic, so the remainder is never taken by
            // zero here. Taking it crashed the interpreter itself.
            IntOut::Bool(if b == 0 { recv == 0 } else { recv % b == 0 })
        }),
        "div_euclid" => arg(args, 0).and_then(|b| {
            if b == 0 {
                bail!("attempt to divide by zero");
            }
            match in_range(width, recv.div_euclid(b)) {
                Some(value) => Ok(IntOut::Same(value)),
                None => bail!("attempt to divide with overflow"),
            }
        }),
        "rem_euclid" => arg(args, 0).and_then(|b| {
            if b == 0 {
                bail!("attempt to calculate the remainder with a divisor of zero");
            }
            match in_range(width, recv.rem_euclid(b)) {
                Some(value) => Ok(IntOut::Same(value)),
                None => bail!("attempt to calculate the remainder with overflow"),
            }
        }),
        "isqrt" => {
            if recv < 0 {
                Err(anyhow::anyhow!(
                    "argument of integer square root cannot be negative"
                ))
            } else {
                Ok(IntOut::Same(isqrt(recv)))
            }
        }
        "count_ones" => Ok(IntOut::Count(raw(width, recv).count_ones())),
        "count_zeros" => Ok(IntOut::Count(bits - raw(width, recv).count_ones())),
        "leading_zeros" => {
            let value = raw(width, recv);
            Ok(IntOut::Count(if value == 0 {
                bits
            } else {
                value.leading_zeros() - (128 - bits)
            }))
        }
        "trailing_zeros" => {
            let value = raw(width, recv);
            Ok(IntOut::Count(if value == 0 {
                bits
            } else {
                value.trailing_zeros()
            }))
        }
        "rotate_left" => count_arg(args, 0).map(|n| IntOut::Same(rotate(width, recv, n, true))),
        "rotate_right" => count_arg(args, 0).map(|n| IntOut::Same(rotate(width, recv, n, false))),
        "swap_bytes" => Ok(IntOut::Same(from_raw(width, swap_bytes(width, recv)))),
        "reverse_bits" => {
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
            low = mid
        } else {
            high = mid - 1
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

    fn same(name: &str, width: IntWidth, recv: i128, args: &[i128]) -> i128 {
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
        let big = u64::MAX as i128;
        assert_eq!(same("max", IntWidth::U64, big, &[0]), big);
        assert_eq!(same("min", IntWidth::U64, big, &[big]), big);
        assert_eq!(same("saturating_add", IntWidth::U64, big, &[0]), big);
    }

    /// Saturation happens at the receiver's real bounds, not at i64's.
    #[test]
    fn saturating_uses_the_real_width() {
        assert_eq!(same("saturating_add", IntWidth::U8, 200, &[100]), 255);
        assert_eq!(same("saturating_sub", IntWidth::I8, -100, &[100]), -128);
        assert_eq!(same("saturating_mul", IntWidth::U8, 5, &[100]), 255);
        assert_eq!(same("saturating_sub", IntWidth::U8, 5, &[100]), 0);
    }

    #[test]
    fn pow_and_abs_panic_where_debug_rust_panics() {
        let overflow = int_method("pow", IntWidth::U8, 16, &[2]).expect("known");
        assert!(overflow.is_err(), "16u8.pow(2) must overflow");
        assert_eq!(same("pow", IntWidth::U8, 15, &[2]), 225);

        let negate = int_method("abs", IntWidth::I8, -128, &[]).expect("known");
        assert!(negate.is_err(), "i8::MIN.abs() must overflow");
        assert_eq!(same("abs", IntWidth::I8, -127, &[]), 127);
    }

    /// A zero divisor here once took the remainder anyway and crashed the
    /// interpreter process with its own host panic.
    #[test]
    fn is_multiple_of_zero_answers_instead_of_crashing() {
        let answer = int_method("is_multiple_of", IntWidth::U64, 0, &[0]).expect("known");
        assert!(matches!(answer, Ok(IntOut::Bool(true))));
        let answer = int_method("is_multiple_of", IntWidth::U64, 5, &[0]).expect("known");
        assert!(matches!(answer, Ok(IntOut::Bool(false))));
    }

    #[test]
    fn wrapping_and_checked_follow_the_width() {
        assert_eq!(same("wrapping_add", IntWidth::U8, 250, &[10]), 4);
        assert_eq!(same("wrapping_sub", IntWidth::U8, 0, &[1]), 255);
        assert_eq!(same("wrapping_mul", IntWidth::I8, 100, &[3]), 44);
        let checked = int_method("checked_add", IntWidth::U8, 250, &[10]).expect("known");
        assert!(matches!(checked, Ok(IntOut::Checked(None))));
        let checked = int_method("checked_add", IntWidth::U8, 1, &[2]).expect("known");
        assert!(matches!(checked, Ok(IntOut::Checked(Some(3)))));
    }

    #[test]
    fn bit_methods_use_the_width_not_the_storage() {
        let count = int_method("count_ones", IntWidth::U8, 250, &[]).expect("known");
        assert!(matches!(count, Ok(IntOut::Count(6))));
        let count = int_method("leading_zeros", IntWidth::U8, 1, &[]).expect("known");
        assert!(matches!(count, Ok(IntOut::Count(7))));
        let count = int_method("trailing_zeros", IntWidth::U8, 0, &[]).expect("known");
        assert!(matches!(count, Ok(IntOut::Count(8))));
        assert_eq!(same("swap_bytes", IntWidth::U16, 0x1234, &[]), 0x3412);
        assert_eq!(same("reverse_bits", IntWidth::U8, 0b1000_0000, &[]), 1);
        assert_eq!(same("rotate_left", IntWidth::U8, 0b1000_0001, &[1]), 0b11);
    }

    #[test]
    fn unknown_names_fall_through() {
        assert!(int_method("sqrt", IntWidth::I64, 4, &[]).is_none());
        assert!(int_method("abs", IntWidth::U8, 4, &[]).is_none());
        assert!(int_method("signum", IntWidth::U8, 4, &[]).is_none());
    }
}
