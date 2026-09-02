//! The char and string method cores, parsing and the `colored` bridge.

use num_traits::PrimInt;
use std::cmp::Ordering;

use anyhow::{Result, anyhow, bail};

use crate::interpreter::bytecode::{BuiltinId, ScalarTy};
use crate::interpreter::numeric::IntWidth;

use super::{Args, int_arg, usize_arg, usize_i64};

pub(crate) enum CharOut {
    Bool(bool),
    Char(char),
    Str(String),
    /// `to_digit`, a u32 payload
    OptU32(Option<u32>),
    USize(usize),
}

pub(crate) fn char_method(ch: char, name: BuiltinId, args: &impl Args) -> Option<Result<CharOut>> {
    let b = |v: bool| Some(Ok(CharOut::Bool(v)));
    match name {
        BuiltinId::ToDigit => {
            let radix = match int_arg(args, 0) {
                Ok(radix) => radix,
                Err(error) => return Some(Err(error)),
            };
            if !(2..=36).contains(&radix) {
                return Some(Err(anyhow!(
                    "to_digit: invalid radix -- radix must be in the range 2 to 36 inclusive"
                )));
            }
            Some(Ok(CharOut::OptU32(
                u32::try_from(radix).ok().and_then(|r| ch.to_digit(r)),
            )))
        }
        BuiltinId::IsAsciiDigit => b(ch.is_ascii_digit()),
        BuiltinId::IsAsciiAlphabetic => b(ch.is_ascii_alphabetic()),
        BuiltinId::IsAsciiAlphanumeric => b(ch.is_ascii_alphanumeric()),
        BuiltinId::IsAsciiUppercase => b(ch.is_ascii_uppercase()),
        BuiltinId::IsAsciiLowercase => b(ch.is_ascii_lowercase()),
        BuiltinId::IsAsciiWhitespace => b(ch.is_ascii_whitespace()),
        BuiltinId::IsAsciiPunctuation => b(ch.is_ascii_punctuation()),
        BuiltinId::IsAsciiHexdigit => b(ch.is_ascii_hexdigit()),
        BuiltinId::IsAscii => b(ch.is_ascii()),
        BuiltinId::IsControl => b(ch.is_control()),
        BuiltinId::EqIgnoreAsciiCase => b(args
            .text(0)
            .chars()
            .next()
            .is_some_and(|other| ch.eq_ignore_ascii_case(&other))),
        BuiltinId::LenUtf8 => Some(Ok(CharOut::USize(ch.len_utf8()))),
        BuiltinId::IsAlphabetic => b(ch.is_alphabetic()),
        BuiltinId::IsAlphanumeric => b(ch.is_alphanumeric()),
        BuiltinId::IsNumeric => b(ch.is_numeric()),
        BuiltinId::IsWhitespace => b(ch.is_whitespace()),
        BuiltinId::IsUppercase => b(ch.is_uppercase()),
        BuiltinId::IsLowercase => b(ch.is_lowercase()),
        BuiltinId::ToAsciiUppercase => Some(Ok(CharOut::Char(ch.to_ascii_uppercase()))),
        BuiltinId::ToAsciiLowercase => Some(Ok(CharOut::Char(ch.to_ascii_lowercase()))),
        // an iterator in real Rust, but a script only prints or collects it
        BuiltinId::ToUppercase => Some(Ok(CharOut::Str(ch.to_uppercase().to_string()))),
        BuiltinId::ToLowercase => Some(Ok(CharOut::Str(ch.to_lowercase().to_string()))),
        _ => None,
    }
}

// strings

/// `Keep` and `OkKeep` give the receiver back, a refcount bump and no copy.
pub(crate) enum StrOut {
    Bool(bool),
    /// with the real `usize` width
    USize(usize),
    Owned(String),
    Keep,
    OkKeep,
    Strs(Vec<String>),
    CharIdx(Vec<(i64, char)>),
    Ints(Vec<i64>),
    OptOwned(Option<String>),
    OptInt(Option<i64>),
    OptPair(Option<(String, String)>),
    Ordering(Ordering),
}

/// `repeat` past `isize::MAX` must be a script panic, not an interpreter death with a different
/// exit code. A count too large for `usize` is a huge count, not zero.
fn str_repeat(s: &str, args: &impl Args) -> Result<String> {
    let n = args
        .int(0)
        .map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX));
    if s.len().saturating_mul(n) > isize::MAX.cast_unsigned() {
        bail!("capacity overflow");
    }
    Ok(s.repeat(n))
}

/// Int first, then float, then bool.
pub(crate) fn str_core(s: &str, name: BuiltinId, args: &impl Args) -> Result<Option<StrOut>> {
    let a = |i: usize| args.text(i);
    Ok(Some(match name {
        BuiltinId::Len => StrOut::USize(s.len()),
        BuiltinId::IsEmpty => StrOut::Bool(s.is_empty()),
        BuiltinId::IsCharBoundary => StrOut::Bool(s.is_char_boundary(usize_arg(args, 0)?)),
        BuiltinId::IsAscii => StrOut::Bool(s.is_ascii()),
        BuiltinId::Count => StrOut::USize(s.chars().count()),
        BuiltinId::Contains => StrOut::Bool(s.contains(&a(0))),
        BuiltinId::EqIgnoreAsciiCase => StrOut::Bool(s.eq_ignore_ascii_case(&a(0))),
        BuiltinId::StartsWith => StrOut::Bool(s.starts_with(&a(0))),
        BuiltinId::EndsWith => StrOut::Bool(s.ends_with(&a(0))),
        BuiltinId::Trim => StrOut::Owned(s.trim().to_string()),
        BuiltinId::TrimStart => StrOut::Owned(s.trim_start().to_string()),
        BuiltinId::TrimEnd => StrOut::Owned(s.trim_end().to_string()),
        BuiltinId::ToUppercase => StrOut::Owned(s.to_uppercase()),
        BuiltinId::ToLowercase => StrOut::Owned(s.to_lowercase()),
        // the ascii variants leave non ascii chars alone
        BuiltinId::ToAsciiUppercase => StrOut::Owned(s.to_ascii_uppercase()),
        BuiltinId::ToAsciiLowercase => StrOut::Owned(s.to_ascii_lowercase()),
        // A char set pattern like `[':', '.']` replaces any member. Otherwise the array renders
        // as text and matches nothing.
        BuiltinId::Replace => match args.pattern_chars(0) {
            Some(cs) => StrOut::Owned(s.replace(cs.as_slice(), &a(1))),
            None => StrOut::Owned(s.replace(&a(0), &a(1))),
        },
        BuiltinId::Replacen => match args.pattern_chars(0) {
            Some(cs) => StrOut::Owned(s.replacen(cs.as_slice(), &a(1), usize_arg(args, 2)?)),
            None => StrOut::Owned(s.replacen(&a(0), &a(1), usize_arg(args, 2)?)),
        },
        BuiltinId::Repeat => StrOut::Owned(str_repeat(s, args)?),
        // A json string is a plain Str, so `unwrap` and `expect` on a string are identity. Keeps
        // serde chains working. `as_ref` and `into_owned` are what the regex replace family needs,
        // it hands back a `Cow<str>` in real Rust and a plain Str here. `to_string_lossy` is the
        // same, an OsStr out of the Path bridge is already valid text.
        BuiltinId::ToStringLossy
        | BuiltinId::ToOwned
        | BuiltinId::TrimString
        | BuiltinId::AsRef
        | BuiltinId::AsStr
        | BuiltinId::AsString
        | BuiltinId::Unwrap
        | BuiltinId::Expect
        | BuiltinId::UnwrapOr
        | BuiltinId::UnwrapOrElse
        | BuiltinId::UnwrapOrDefault
        | BuiltinId::IntoOwned
        | BuiltinId::IntoString => StrOut::Keep,
        // an OsStr out of the Path bridge is a plain string, so its conversions land here
        BuiltinId::ToStr => StrOut::OptOwned(Some(s.to_string())),
        // `Option::context` returns a Result, otherwise a following `?` has nothing to unwrap
        BuiltinId::Context | BuiltinId::WithContext => StrOut::OkKeep,
        BuiltinId::IsSome => StrOut::Bool(true),
        BuiltinId::IsNone => StrOut::Bool(false),
        BuiltinId::AsBytes | BuiltinId::IntoBytes => {
            StrOut::Ints(s.bytes().map(i64::from).collect())
        }
        BuiltinId::EncodeUtf16 => StrOut::Ints(s.encode_utf16().map(i64::from).collect()),
        BuiltinId::StripPrefix => StrOut::OptOwned(s.strip_prefix(&a(0)).map(str::to_string)),
        BuiltinId::StripSuffix => StrOut::OptOwned(s.strip_suffix(&a(0)).map(str::to_string)),
        // byte offsets like std, so `&s[..s.find(x).unwrap()]` works
        BuiltinId::Find => StrOut::OptInt(s.find(&a(0)).map(usize_i64)),
        BuiltinId::Rfind => StrOut::OptInt(s.rfind(&a(0)).map(usize_i64)),
        BuiltinId::SplitOnce => StrOut::OptPair(
            s.split_once(&a(0))
                .map(|(x, y)| (x.to_string(), y.to_string())),
        ),
        BuiltinId::RsplitOnce => StrOut::OptPair(
            s.rsplit_once(&a(0))
                .map(|(x, y)| (x.to_string(), y.to_string())),
        ),
        // a char array splits on any of its members
        BuiltinId::Split => match args.pattern_chars(0) {
            Some(chars) => StrOut::Strs(
                s.split(|c: char| chars.contains(&c))
                    .map(str::to_string)
                    .collect(),
            ),
            None => StrOut::Strs(s.split(&a(0)).map(str::to_string).collect()),
        },
        BuiltinId::Rsplit => StrOut::Strs(s.rsplit(&a(0)).map(str::to_string).collect()),
        BuiltinId::Splitn => {
            let n = usize_arg(args, 0)?;
            StrOut::Strs(s.splitn(n, &a(1)).map(str::to_string).collect())
        }
        BuiltinId::Rsplitn => {
            let n = usize_arg(args, 0)?;
            StrOut::Strs(s.rsplitn(n, &a(1)).map(str::to_string).collect())
        }
        BuiltinId::Matches => StrOut::Strs(s.matches(&a(0)).map(str::to_string).collect()),
        BuiltinId::CharIndices => {
            StrOut::CharIdx(s.char_indices().map(|(i, c)| (usize_i64(i), c)).collect())
        }
        BuiltinId::TrimMatches | BuiltinId::TrimStartMatches | BuiltinId::TrimEndMatches => {
            let pat = a(0);
            let out = match name {
                BuiltinId::TrimStartMatches => s.trim_start_matches(&pat),
                BuiltinId::TrimEndMatches => s.trim_end_matches(&pat),
                // `trim_matches` only takes chars
                _ => match args.pattern_chars(0) {
                    Some(chars) => s.trim_matches(|c: char| chars.contains(&c)),
                    None => s.trim_matches(pat.chars().next().unwrap_or(' ')),
                },
            };
            StrOut::Owned(out.to_string())
        }
        BuiltinId::Cmp => StrOut::Ordering(s.cmp(a(0).as_str())),
        // `parse_core` is the only place that knows the target type
        _ => return Ok(None),
    }))
}

pub(crate) enum Parsed {
    Int(i128, IntWidth),
    F32(f32),
    F64(f64),
    Bool(bool),
    Char(char),
    Str(String),
    Fail(String),
}

/// Written out by hand because std has no way to build a `ParseIntError`. Every other parse
/// failure carries the real error.
fn out_of_range(too_small: bool) -> String {
    if too_small {
        "number too small to fit in target type".to_string()
    } else {
        "number too large to fit in target type".to_string()
    }
}

fn int_error(text: &str) -> String {
    text.parse::<i64>()
        .err()
        .map_or_else(|| format!("cannot parse `{text}`"), |e| e.to_string())
}

/// Uses the target type when the call wrote one. Guessing made `"300".parse::<u8>()` an
/// `Ok(300)`. Without a turbofish we still guess.
pub(crate) fn parse_core(text: &str, target: Option<&ScalarTy>) -> Parsed {
    let fail = |e: &dyn std::fmt::Display| Parsed::Fail(e.to_string());
    let Some(target) = target else {
        let trimmed = text.trim();
        return if let Ok(value) = trimmed.parse::<i64>() {
            Parsed::Int(i128::from(value), IntWidth::I64)
        } else if let Ok(value) = trimmed.parse::<u128>() {
            // an integer past i64 keeps its digits at 128 bits
            Parsed::Int(value.cast_signed(), IntWidth::U128)
        } else if let Ok(value) = trimmed.parse::<i128>() {
            Parsed::Int(value, IntWidth::I128)
        } else if let Ok(value) = trimmed.parse::<f64>() {
            Parsed::F64(value)
        } else if let Ok(value) = trimmed.parse::<bool>() {
            Parsed::Bool(value)
        } else {
            // report what an integer parse would say, the real std message
            Parsed::Fail(int_error(trimmed))
        };
    };
    match target {
        ScalarTy::Int(IntWidth::U128) => {
            match parse_int_digits::<u128>(text, false, 0, u128::MAX) {
                Ok(value) => Parsed::Int(value.cast_signed(), IntWidth::U128),
                Err(message) => Parsed::Fail(message),
            }
        }
        ScalarTy::Int(width) => {
            match parse_int_digits::<i128>(text, width.is_signed(), width.min(), width.max()) {
                Ok(value) => Parsed::Int(value, *width),
                Err(message) => Parsed::Fail(message),
            }
        }
        ScalarTy::F32 => text.parse::<f32>().map_or_else(|e| fail(&e), Parsed::F32),
        ScalarTy::F64 => text.parse::<f64>().map_or_else(|e| fail(&e), Parsed::F64),
        ScalarTy::Bool => text.parse::<bool>().map_or_else(|e| fail(&e), Parsed::Bool),
        ScalarTy::Char => text.parse::<char>().map_or_else(|e| fail(&e), Parsed::Char),
        ScalarTy::Str => Parsed::Str(text.to_string()),
        // no container implements `FromStr`, these only describe a `Default`
        ScalarTy::Opt(_)
        | ScalarTy::List(_)
        | ScalarTy::Map(_)
        | ScalarTy::Set(_)
        | ScalarTy::Other => Parsed::Fail(format!("cannot parse `{text}`")),
    }
}

/// The `colored` crate as string methods. Returns a plain string with ANSI codes so chaining
/// works. Honors `NO_COLOR` and terminal detection.
pub(crate) fn color_core(s: &str, name: BuiltinId) -> Option<String> {
    use colored::Colorize;
    let out = match name {
        BuiltinId::Red => s.red(),
        BuiltinId::Green => s.green(),
        BuiltinId::Yellow => s.yellow(),
        BuiltinId::Blue => s.blue(),
        BuiltinId::Magenta | BuiltinId::Purple => s.magenta(),
        BuiltinId::Cyan => s.cyan(),
        BuiltinId::White => s.white(),
        BuiltinId::Black => s.black(),
        BuiltinId::BrightRed => s.bright_red(),
        BuiltinId::BrightGreen => s.bright_green(),
        BuiltinId::BrightYellow => s.bright_yellow(),
        BuiltinId::BrightBlue => s.bright_blue(),
        BuiltinId::BrightCyan => s.bright_cyan(),
        BuiltinId::OnRed => s.on_red(),
        BuiltinId::OnGreen => s.on_green(),
        BuiltinId::OnBlue => s.on_blue(),
        BuiltinId::Bold => s.bold(),
        BuiltinId::Dimmed => s.dimmed(),
        BuiltinId::Italic => s.italic(),
        BuiltinId::Underline => s.underline(),
        BuiltinId::Reversed => s.reversed(),
        BuiltinId::Clear | BuiltinId::Normal => s.normal(),
        _ => return None,
    };
    Some(out.to_string())
}

/// The digit loop of `from_str_radix`, so the first error in reading order wins the way it does
/// in std. `"99999999999999999999\n".parse::<usize>()` overflows before the newline is seen, and
/// `"-1".parse::<u8>()` rejects the sign as a digit. The bounds stand in for the target width.
fn parse_int_digits<T: PrimInt>(text: &str, signed: bool, low: T, high: T) -> Result<T, String> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return Err("cannot parse integer from empty string".to_string());
    }
    let (positive, digits) = match bytes {
        [b'+' | b'-'] => return Err("invalid digit found in string".to_string()),
        [b'+', rest @ ..] => (true, rest),
        [b'-', rest @ ..] if signed => (false, rest),
        _ => (true, bytes),
    };
    let overflow = || Err(out_of_range(!positive));
    let radix = T::from(10).expect("10 fits every integer");
    let mut result = T::zero();
    for byte in digits {
        let scaled = result
            .checked_mul(&radix)
            .filter(|v| *v >= low && *v <= high);
        let Some(digit) = (*byte as char).to_digit(10) else {
            return Err("invalid digit found in string".to_string());
        };
        let Some(scaled) = scaled else {
            return overflow();
        };
        let digit = T::from(digit).expect("a digit fits every integer");
        let next = if positive {
            scaled.checked_add(&digit)
        } else {
            scaled.checked_sub(&digit)
        };
        match next.filter(|v| *v >= low && *v <= high) {
            Some(next) => result = next,
            None => return overflow(),
        }
    }
    Ok(result)
}
