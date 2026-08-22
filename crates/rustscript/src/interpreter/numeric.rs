//! Width aware integer semantics. A tagged value lives in 1 i64. Widths up to u32 store the true
//! value, `U64` and `USize` store the raw bits. `I64` never appears in a tag.

use num_traits::AsPrimitive;
use std::ops::{Add, Div, Mul, Rem, Sub};

use anyhow::{Result, anyhow, bail};

use super::bytecode::{BinKind, overflow_message};

/// `I64` doubles as the width of an untagged value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntWidth {
    U8,
    U16,
    U32,
    U64,
    USize,
    /// stored in a `Value::Big`
    U128,
    I8,
    I16,
    I32,
    I64,
    /// stored in a `Value::Big`
    I128,
}

impl IntWidth {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "u8" => Self::U8,
            "u16" => Self::U16,
            "u32" => Self::U32,
            "u64" => Self::U64,
            "u128" => Self::U128,
            "usize" => Self::USize,
            "i8" => Self::I8,
            "i16" => Self::I16,
            "i32" => Self::I32,
            "i128" => Self::I128,
            // 64 bit targets only, so isize is i64
            "i64" | "isize" => Self::I64,
            _ => return None,
        })
    }

    /// `I64` also covers isize and `USize` also covers u64.
    pub fn name(self) -> &'static str {
        match self {
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::U128 => "u128",
            Self::USize => "usize",
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::I128 => "i128",
        }
    }

    pub fn is_signed(self) -> bool {
        matches!(
            self,
            Self::I8 | Self::I16 | Self::I32 | Self::I64 | Self::I128
        )
    }

    pub fn is_big(self) -> bool {
        matches!(self, Self::I128 | Self::U128)
    }

    pub fn bits(self) -> u32 {
        match self {
            Self::U8 | Self::I8 => 8,
            Self::U16 | Self::I16 => 16,
            Self::U32 | Self::I32 => 32,
            Self::U64 | Self::USize | Self::I64 => 64,
            Self::U128 | Self::I128 => 128,
        }
    }

    /// `U128` never asks, its arithmetic runs natively.
    pub fn min(self) -> i128 {
        match self {
            Self::I128 => i128::MIN,
            _ if self.is_signed() => -(1i128 << (self.bits() - 1)),
            _ => 0,
        }
    }

    /// `U128` never asks, its bound doesn't fit an i128.
    pub fn max(self) -> i128 {
        match self {
            Self::I128 => i128::MAX,
            Self::U128 => unreachable!("u128 bounds do not fit the i128 pipeline"),
            _ if self.is_signed() => (1i128 << (self.bits() - 1)) - 1,
            _ => (1i128 << self.bits()) - 1,
        }
    }

    pub fn decode(self, stored: i64) -> i128 {
        match self {
            Self::U64 | Self::USize => i128::from(stored.cast_unsigned()),
            Self::U128 | Self::I128 => unreachable!("128-bit values live in Value::Big"),
            _ => i128::from(stored),
        }
    }

    pub fn encode(self, value: i128) -> i64 {
        match self {
            Self::U64 | Self::USize => AsPrimitive::<u64>::as_(value).cast_signed(),
            Self::U128 | Self::I128 => unreachable!("128-bit values live in Value::Big"),
            _ => AsPrimitive::<i64>::as_(value),
        }
    }
}

/// 128 bit ops, natively checked. `U128` bits are decoded here.
pub fn big_arith(op: BinKind, width: IntWidth, a: i128, b: i128) -> Result<i128> {
    if width == IntWidth::U128 {
        let (x, y) = (a.cast_unsigned(), b.cast_unsigned());
        let out: u128 = match op {
            BinKind::Add => x
                .checked_add(y)
                .ok_or_else(|| anyhow!("{}", overflow_message(op)))?,
            BinKind::Sub => x
                .checked_sub(y)
                .ok_or_else(|| anyhow!("{}", overflow_message(op)))?,
            BinKind::Mul => x
                .checked_mul(y)
                .ok_or_else(|| anyhow!("{}", overflow_message(op)))?,
            BinKind::Div => {
                if y == 0 {
                    bail!("attempt to divide by zero");
                }
                x / y
            }
            BinKind::Rem => {
                if y == 0 {
                    bail!("attempt to calculate the remainder with a divisor of zero");
                }
                x % y
            }
            BinKind::BitAnd => x & y,
            BinKind::BitOr => x | y,
            BinKind::BitXor => x ^ y,
            _ => bail!("not an arithmetic operator"),
        };
        return Ok(out.cast_signed());
    }
    Ok(match op {
        BinKind::Add => a
            .checked_add(b)
            .ok_or_else(|| anyhow!("{}", overflow_message(op)))?,
        BinKind::Sub => a
            .checked_sub(b)
            .ok_or_else(|| anyhow!("{}", overflow_message(op)))?,
        BinKind::Mul => a
            .checked_mul(b)
            .ok_or_else(|| anyhow!("{}", overflow_message(op)))?,
        BinKind::Div => {
            if b == 0 {
                bail!("attempt to divide by zero");
            }
            a.checked_div(b)
                .ok_or_else(|| anyhow!("{}", overflow_message(op)))?
        }
        BinKind::Rem => {
            if b == 0 {
                bail!("attempt to calculate the remainder with a divisor of zero");
            }
            a.checked_rem(b)
                .ok_or_else(|| anyhow!("{}", overflow_message(op)))?
        }
        BinKind::BitAnd => a & b,
        BinKind::BitOr => a | b,
        BinKind::BitXor => a ^ b,
        _ => bail!("not an arithmetic operator"),
    })
}

/// An untagged side is a bare literal adopting the other width, u64 and usize share 1 semantic.
/// Anything else can't pass the type checker.
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
            // `MIN % -1` is 0 in i128 but overflows in the real width
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

/// The native fast path of the 64 bit unsigned widths.
#[inline]
pub fn u64_arith(op: BinKind, a: u64, b: u64) -> Result<u64> {
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
            a / b
        }
        BinKind::Rem => {
            if b == 0 {
                bail!("attempt to calculate the remainder with a divisor of zero");
            }
            a % b
        }
        _ => unreachable!(),
    })
}

/// The hot fast path of the VM, checked native arithmetic with no i128 widening.
#[inline]
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

#[inline]
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

/// The amount never unifies with the shifted side. An amount at the bit count panics, bits
/// shifted out are discarded.
pub fn int_shift(op: BinKind, width: IntWidth, value: i128, amount: i128) -> Result<i128> {
    let (verb, left) = match op {
        BinKind::Shl => ("left", true),
        BinKind::Shr => ("right", false),
        _ => bail!("not a shift operator"),
    };
    if amount < 0 || amount >= i128::from(width.bits()) {
        bail!("attempt to shift {verb} with overflow");
    }
    // u128 shifts logically, an arithmetic i128 shift would smear the sign bit
    if width == IntWidth::U128 {
        let bits = value.cast_unsigned();
        let shifted = if left { bits << amount } else { bits >> amount };
        return Ok(shifted.cast_signed());
    }
    let shifted = if left {
        truncate(value << amount, width)
    } else {
        value >> amount
    };
    Ok(shifted)
}

/// Only signed widths implement negation.
pub fn int_neg(width: IntWidth, value: i128) -> Result<i128> {
    if !width.is_signed() {
        bail!("cannot negate an unsigned integer");
    }
    if value == width.min() {
        bail!("attempt to negate with overflow");
    }
    Ok(-value)
}

/// Two's complement on i128 agrees with the real width, only `!` needs a truncation.
pub fn int_bit(op: BinKind, a: i128, b: i128) -> Result<i128> {
    Ok(match op {
        BinKind::BitAnd => a & b,
        BinKind::BitOr => a | b,
        BinKind::BitXor => a ^ b,
        _ => bail!("not a bitwise operator"),
    })
}

pub fn int_not(width: IntWidth, value: i128) -> i128 {
    truncate(!value, width)
}

/// Keep the low bits, reinterpret in the target, the host's own cast per width.
pub fn truncate(value: i128, target: IntWidth) -> i128 {
    match target {
        IntWidth::U8 => i128::from(AsPrimitive::<u8>::as_(value)),
        IntWidth::U16 => i128::from(AsPrimitive::<u16>::as_(value)),
        IntWidth::U32 => i128::from(AsPrimitive::<u32>::as_(value)),
        IntWidth::U64 | IntWidth::USize => i128::from(AsPrimitive::<u64>::as_(value)),
        IntWidth::I8 => i128::from(AsPrimitive::<i8>::as_(value)),
        IntWidth::I16 => i128::from(AsPrimitive::<i16>::as_(value)),
        IntWidth::I32 => i128::from(AsPrimitive::<i32>::as_(value)),
        IntWidth::I64 => i128::from(AsPrimitive::<i64>::as_(value)),
        // the 128 bit widths keep the whole i128
        IntWidth::U128 | IntWidth::I128 => value,
    }
}

/// The host's own cast has exactly these semantics, so delegate per width.
pub fn float_to_int(value: f64, target: IntWidth) -> i128 {
    match target {
        IntWidth::U8 => i128::from(AsPrimitive::<u8>::as_(value)),
        IntWidth::U16 => i128::from(AsPrimitive::<u16>::as_(value)),
        IntWidth::U32 => i128::from(AsPrimitive::<u32>::as_(value)),
        IntWidth::U64 | IntWidth::USize => i128::from(AsPrimitive::<u64>::as_(value)),
        IntWidth::I8 => i128::from(AsPrimitive::<i8>::as_(value)),
        IntWidth::I16 => i128::from(AsPrimitive::<i16>::as_(value)),
        IntWidth::I32 => i128::from(AsPrimitive::<i32>::as_(value)),
        IntWidth::I64 => i128::from(AsPrimitive::<i64>::as_(value)),
        IntWidth::I128 => AsPrimitive::<i128>::as_(value),
        IntWidth::U128 => AsPrimitive::<u128>::as_(value).cast_signed(),
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
        assert_eq!(truncate(-1, IntWidth::U64), i128::from(u64::MAX));
        assert_eq!(float_to_int(300.9, IntWidth::U8), 255);
        assert_eq!(float_to_int(f64::NAN, IntWidth::I32), 0);
        assert_eq!(float_to_int(-1.5, IntWidth::U16), 0);
    }
}
