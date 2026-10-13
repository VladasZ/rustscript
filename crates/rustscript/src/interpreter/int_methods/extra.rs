//! The divisions that saturate or wrap, the logarithm with a base, the endian conversions and
//! the methods that cross between the signed and the unsigned type of 1 size.

use anyhow::{Result, anyhow, bail};
use num_traits::AsPrimitive;

use super::{ByteOrder, IntOut, arg, from_raw, in_range, raw, saturate, swap_bytes};
use crate::interpreter::bytecode::BuiltinId;
use crate::interpreter::numeric::IntWidth;

const LOG_NOT_POSITIVE: &str = "argument of integer logarithm must be positive";

pub(super) fn int_extra_family(
    name: BuiltinId,
    width: IntWidth,
    recv: i128,
    args: &[i128],
) -> Option<Result<IntOut>> {
    if let Some(out) = int_sign_family(name, width, recv) {
        return Some(Ok(out));
    }
    let out = match name {
        BuiltinId::SaturatingDiv => arg(args, 0).and_then(|b| {
            if b == 0 {
                bail!("attempt to divide by zero");
            }
            Ok(IntOut::Same(saturate(width, recv / b)))
        }),
        // `MIN / -1` leaves the width, the i128 the value sits in holds it
        BuiltinId::WrappingDiv | BuiltinId::WrappingDivEuclid => arg(args, 0).and_then(|b| {
            if b == 0 {
                bail!("attempt to divide by zero");
            }
            let exact = if name == BuiltinId::WrappingDiv {
                recv / b
            } else {
                recv.div_euclid(b)
            };
            Ok(IntOut::Same(from_raw(
                width,
                AsPrimitive::<u128>::as_(exact),
            )))
        }),
        // `MIN % -1` is 0 in an i128, which is the wrapped answer
        BuiltinId::WrappingRem | BuiltinId::WrappingRemEuclid => arg(args, 0).and_then(|b| {
            if b == 0 {
                bail!("attempt to calculate the remainder with a divisor of zero");
            }
            Ok(IntOut::Same(if name == BuiltinId::WrappingRem {
                recv % b
            } else {
                recv.rem_euclid(b)
            }))
        }),
        BuiltinId::CheckedDivEuclid => arg(args, 0).map(|b| {
            IntOut::Checked(if b == 0 {
                None
            } else {
                in_range(width, recv.div_euclid(b))
            })
        }),
        BuiltinId::CheckedIlog10 => Ok(IntOut::CheckedCount(recv.checked_ilog10())),
        // std looks at the base first
        BuiltinId::Ilog => arg(args, 0).and_then(|base| {
            if base < 2 {
                bail!("base of integer logarithm must be at least 2");
            }
            match recv.checked_ilog(base) {
                Some(count) => Ok(IntOut::Count(count)),
                None => Err(anyhow!(LOG_NOT_POSITIVE)),
            }
        }),
        BuiltinId::CheckedIlog => {
            arg(args, 0).map(|base| IntOut::CheckedCount(recv.checked_ilog(base)))
        }
        BuiltinId::ToLe | BuiltinId::ToBe => {
            let swaps = (name == BuiltinId::ToBe) == ByteOrder::Ne.little();
            Ok(IntOut::Same(if swaps {
                from_raw(width, swap_bytes(width, recv))
            } else {
                recv
            }))
        }
        _ => return None,
    };
    Some(out)
}

/// Each exists on 1 signedness only.
fn int_sign_family(name: BuiltinId, width: IntWidth, recv: i128) -> Option<IntOut> {
    let signed = width.is_signed();
    Some(match name {
        BuiltinId::UnsignedAbs if signed => IntOut::Unsigned(recv.abs()),
        BuiltinId::SaturatingAbs if signed => IntOut::Same(saturate(width, recv.abs())),
        BuiltinId::SaturatingNeg if signed => IntOut::Same(saturate(width, -recv)),
        BuiltinId::CastUnsigned if signed => {
            IntOut::Unsigned(AsPrimitive::<i128>::as_(raw(width, recv)))
        }
        BuiltinId::CastSigned if !signed => {
            IntOut::Signed(from_raw(width.signed(), raw(width, recv)))
        }
        _ => return None,
    })
}
