//! The ASCII helpers of `u8`. Real Rust has them on `u8` alone, so a receiver that passed
//! `cargo check` is a byte whatever width its value carries here.

use anyhow::Result;

use super::{IntOut, arg};
use crate::interpreter::bytecode::BuiltinId;

pub(super) fn int_ascii_family(
    name: BuiltinId,
    recv: i128,
    args: &[i128],
) -> Option<Result<IntOut>> {
    let byte = u8::try_from(recv).ok()?;
    let out = match name {
        BuiltinId::IsAscii => IntOut::Bool(byte.is_ascii()),
        BuiltinId::IsAsciiAlphabetic => IntOut::Bool(byte.is_ascii_alphabetic()),
        BuiltinId::IsAsciiAlphanumeric => IntOut::Bool(byte.is_ascii_alphanumeric()),
        BuiltinId::IsAsciiControl => IntOut::Bool(byte.is_ascii_control()),
        BuiltinId::IsAsciiDigit => IntOut::Bool(byte.is_ascii_digit()),
        BuiltinId::IsAsciiGraphic => IntOut::Bool(byte.is_ascii_graphic()),
        BuiltinId::IsAsciiHexdigit => IntOut::Bool(byte.is_ascii_hexdigit()),
        BuiltinId::IsAsciiLowercase => IntOut::Bool(byte.is_ascii_lowercase()),
        BuiltinId::IsAsciiPunctuation => IntOut::Bool(byte.is_ascii_punctuation()),
        BuiltinId::IsAsciiUppercase => IntOut::Bool(byte.is_ascii_uppercase()),
        BuiltinId::IsAsciiWhitespace => IntOut::Bool(byte.is_ascii_whitespace()),
        BuiltinId::ToAsciiUppercase => IntOut::Same(i128::from(byte.to_ascii_uppercase())),
        BuiltinId::ToAsciiLowercase => IntOut::Same(i128::from(byte.to_ascii_lowercase())),
        BuiltinId::EqIgnoreAsciiCase => {
            return Some(arg(args, 0).map(|other| {
                IntOut::Bool(
                    u8::try_from(other).is_ok_and(|other| byte.eq_ignore_ascii_case(&other)),
                )
            }));
        }
        _ => return None,
    };
    Some(Ok(out))
}
