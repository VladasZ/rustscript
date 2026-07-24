//! Width-aware integer semantics shared by both engines. Values carry their
//! real Rust integer width at runtime, so arithmetic panics exactly where
//! debug Rust panics, casts truncate and saturate the same way, and u64 and
//! usize keep their full range.
//!
//! Storage convention: a width-tagged value lives in one i64. Signed widths
//! and unsigned widths up to u32 store the true value. U64 and USize store
//! the raw bits, reinterpreted through `u64` on decode. `I64` never appears
//! in a tag, a plain i64 stays the engine's untagged integer value.

use std::ops::{Add, Div, Mul, Rem, Sub};

use anyhow::{Result, anyhow, bail};

use super::bytecode::{BinKind, overflow_message};

/// Every integer width real Rust has on a 64-bit target. `I64` doubles as
/// the width of an untagged value, which is also what a bare literal carries
/// until an operation with a tagged operand adopts its width.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntWidth {
    U8,
    U16,
    U32,
    U64,
    USize,
    I8,
    I16,
    I32,
    I64,
}

impl IntWidth {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "u8" => Self::U8,
            "u16" => Self::U16,
            "u32" => Self::U32,
            "u64" => Self::U64,
            "usize" => Self::USize,
            "i8" => Self::I8,
            "i16" => Self::I16,
            "i32" => Self::I32,
            // The engines run on 64-bit targets only, so isize is i64.
            "i64" | "isize" => Self::I64,
            _ => return None,
        })
    }

    pub fn is_signed(self) -> bool {
        matches!(self, Self::I8 | Self::I16 | Self::I32 | Self::I64)
    }

    pub fn bits(self) -> u32 {
        match self {
            Self::U8 | Self::I8 => 8,
            Self::U16 | Self::I16 => 16,
            Self::U32 | Self::I32 => 32,
            Self::U64 | Self::USize | Self::I64 => 64,
        }
    }

    pub fn min(self) -> i128 {
        if self.is_signed() {
            -(1i128 << (self.bits() - 1))
        } else {
            0
        }
    }

    pub fn max(self) -> i128 {
        if self.is_signed() {
            (1i128 << (self.bits() - 1)) - 1
        } else {
            (1i128 << self.bits()) - 1
        }
    }

    /// Decode a stored i64 into the value it represents.
    pub fn decode(self, stored: i64) -> i128 {
        match self {
            Self::U64 | Self::USize => (stored as u64) as i128,
            _ => stored as i128,
        }
    }

    /// Encode an in-range value into its i64 storage form.
    pub fn encode(self, value: i128) -> i64 {
        match self {
            Self::U64 | Self::USize => (value as u64) as i64,
            _ => value as i64,
        }
    }
}

/// The width two operands of one binary op compute in. Equal widths agree,
/// an untagged i64 side is a bare literal adopting the other side's width,
/// and u64 with usize share one 64-bit unsigned semantic. Anything else
/// cannot appear in a program that passed the real type checker.
pub fn unify(a: IntWidth, b: IntWidth) -> Result<IntWidth> {
    if a == b || b == IntWidth::I64 {
        return Ok(a);
    }
    if a == IntWidth::I64 {
        return Ok(b);
    }
    if matches!(a, IntWidth::U64 | IntWidth::USize) && matches!(b, IntWidth::U64 | IntWidth::USize)
    {
        return Ok(a);
    }
    bail!("cannot mix integer widths in one operation")
}

/// `+ - * / %` in a real width, panicking exactly like debug Rust.
pub fn int_arith(op: BinKind, width: IntWidth, a: i128, b: i128) -> Result<i128> {
    let result = match op {
        BinKind::Add => a + b,
        BinKind::Sub => a - b,
        BinKind::Mul => a * b,
        BinKind::Div => {
            if b == 0 {
                bail!("attempt to divide by zero");
            }
            a / b
        }
        BinKind::Rem => {
            if b == 0 {
                bail!("attempt to calculate the remainder with a divisor of zero");
            }
            // MIN % -1 is 0 in i128 but overflows in the real width.
            if a == width.min() && b == -1 {
                bail!("{}", overflow_message(op));
            }
            a % b
        }
        _ => bail!("not an arithmetic operator"),
    };
    if result < width.min() || result > width.max() {
        bail!("{}", overflow_message(op));
    }
    Ok(result)
}

/// `+ - * / %` on untagged i64 values, panicking exactly like debug Rust.
/// The hot fast path of both engines, so it stays checked native arithmetic
/// with no i128 widening.
#[inline(always)]
pub fn i64_arith(op: BinKind, a: i64, b: i64) -> Result<i64> {
    Ok(match op {
        BinKind::Add => a
            .checked_add(b)
            .ok_or_else(|| anyhow!("attempt to add with overflow"))?,
        BinKind::Sub => a
            .checked_sub(b)
            .ok_or_else(|| anyhow!("attempt to subtract with overflow"))?,
        BinKind::Mul => a
            .checked_mul(b)
            .ok_or_else(|| anyhow!("attempt to multiply with overflow"))?,
        BinKind::Div => {
            if b == 0 {
                bail!("attempt to divide by zero");
            }
            a.checked_div(b)
                .ok_or_else(|| anyhow!("attempt to divide with overflow"))?
        }
        BinKind::Rem => {
            if b == 0 {
                bail!("attempt to calculate the remainder with a divisor of zero");
            }
            a.checked_rem(b)
                .ok_or_else(|| anyhow!("attempt to calculate the remainder with overflow"))?
        }
        _ => unreachable!(),
    })
}

/// `+ - * / %` at one float width. Rust float arithmetic never panics.
#[inline(always)]
pub fn float_arith<T>(op: BinKind, x: T, y: T) -> T
where
    T: Add<Output = T> + Sub<Output = T> + Mul<Output = T> + Div<Output = T> + Rem<Output = T>,
{
    match op {
        BinKind::Add => x + y,
        BinKind::Sub => x - y,
        BinKind::Mul => x * y,
        BinKind::Div => x / y,
        BinKind::Rem => x % y,
        _ => unreachable!(),
    }
}

/// `<<` and `>>`. The amount carries its own width and never unifies with
/// the shifted side. An amount at or past the width's bit count panics like
/// debug Rust, and bits shifted out are discarded like release Rust.
pub fn int_shift(op: BinKind, width: IntWidth, value: i128, amount: i128) -> Result<i128> {
    let (verb, left) = match op {
        BinKind::Shl => ("left", true),
        BinKind::Shr => ("right", false),
        _ => bail!("not a shift operator"),
    };
    if amount < 0 || amount >= i128::from(width.bits()) {
        bail!("attempt to shift {verb} with overflow");
    }
    let shifted = if left {
        truncate(value << amount, width)
    } else {
        value >> amount
    };
    Ok(shifted)
}

/// `-x`. Only signed widths implement negation in real Rust.
pub fn int_neg(width: IntWidth, value: i128) -> Result<i128> {
    if !width.is_signed() {
        bail!("cannot negate an unsigned integer");
    }
    if value == width.min() {
        bail!("attempt to negate with overflow");
    }
    Ok(-value)
}

/// `& | ^` on two same-width operands. Two's complement on i128 agrees with
/// the real width for canonical values, only `!` needs a truncation.
pub fn int_bit(op: BinKind, a: i128, b: i128) -> Result<i128> {
    Ok(match op {
        BinKind::BitAnd => a & b,
        BinKind::BitOr => a | b,
        BinKind::BitXor => a ^ b,
        _ => bail!("not a bitwise operator"),
    })
}

/// `!x` in a real width.
pub fn int_not(width: IntWidth, value: i128) -> i128 {
    truncate(!value, width)
}

/// An `as` cast between integer widths: keep the low bits, reinterpret in
/// the target, exactly the host's own cast per width.
pub fn truncate(value: i128, target: IntWidth) -> i128 {
    match target {
        IntWidth::U8 => value as u8 as i128,
        IntWidth::U16 => value as u16 as i128,
        IntWidth::U32 => value as u32 as i128,
        IntWidth::U64 | IntWidth::USize => value as u64 as i128,
        IntWidth::I8 => value as i8 as i128,
        IntWidth::I16 => value as i16 as i128,
        IntWidth::I32 => value as i32 as i128,
        IntWidth::I64 => value as i64 as i128,
    }
}

/// A float to integer `as` cast: truncate toward zero, saturate at the
/// bounds, NaN becomes zero. The host's own cast has exactly these
/// semantics, so delegate per width.
pub fn float_to_int(value: f64, target: IntWidth) -> i128 {
    match target {
        IntWidth::U8 => value as u8 as i128,
        IntWidth::U16 => value as u16 as i128,
        IntWidth::U32 => value as u32 as i128,
        IntWidth::U64 | IntWidth::USize => value as u64 as i128,
        IntWidth::I8 => value as i8 as i128,
        IntWidth::I16 => value as i16 as i128,
        IntWidth::I32 => value as i32 as i128,
        IntWidth::I64 => value as i64 as i128,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arith_panics_on_the_width_boundary() {
        assert_eq!(int_arith(BinKind::Add, IntWidth::U8, 200, 55).unwrap(), 255);
        assert!(int_arith(BinKind::Add, IntWidth::U8, 200, 56).is_err());
        assert_eq!(
            int_arith(BinKind::Mul, IntWidth::U64, 1 << 62, 3).unwrap(),
            3 << 62
        );
        assert!(int_arith(BinKind::Rem, IntWidth::I8, -128, -1).is_err());
    }

    #[test]
    fn shifts_check_the_amount_not_the_value() {
        assert_eq!(
            int_shift(BinKind::Shl, IntWidth::U8, 255, 4).unwrap(),
            0b1111_0000
        );
        assert!(int_shift(BinKind::Shl, IntWidth::U8, 1, 8).is_err());
        assert_eq!(int_shift(BinKind::Shr, IntWidth::I8, -128, 1).unwrap(), -64);
    }

    #[test]
    fn casts_truncate_and_saturate() {
        assert_eq!(truncate(300, IntWidth::U8), 44);
        assert_eq!(truncate(-1, IntWidth::U64), u64::MAX as i128);
        assert_eq!(float_to_int(300.9, IntWidth::U8), 255);
        assert_eq!(float_to_int(f64::NAN, IntWidth::I32), 0);
        assert_eq!(float_to_int(-1.5, IntWidth::U16), 0);
    }
}
