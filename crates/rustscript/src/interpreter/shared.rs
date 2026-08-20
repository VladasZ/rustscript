//! Shared method cores for scalar receivers.
//!
//! A core works on plain Rust types and answers through a small output
//! enum, so the dispatch layer only adapts arguments in and values out, and
//! the coverage harvest reads each core exactly once.
//!
//! What stays out of the cores: anything lazy or stateful. The
//! iterator forms of `chars`, `lines`, `bytes`, and `split_whitespace` cannot
//! be expressed as a finished value.

use num_traits::AsPrimitive;
use std::cmp::Ordering;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};

use super::bytecode::{BinKind, BuiltinId, BuiltinId as B, ScalarTy};
use super::numeric::IntWidth;

/// View of a method's arguments. The dispatch layer adapts the value slice,
/// and the cores monomorphize over this, so the view costs nothing.
pub(super) trait Args {
    /// The argument rendered as text, what `Display` would print. Missing
    /// arguments render empty, the behavior scripts always saw.
    fn text(&self, i: usize) -> String;
    fn int(&self, i: usize) -> Option<i64>;
    /// An integer, or an integer view of a float argument.
    fn float(&self, i: usize) -> Option<f64>;
    /// The chars of a `['-', '_']` style pattern array argument, so a char
    /// set splits on any of its members rather than the rendered text.
    fn pattern_chars(&self, i: usize) -> Option<Vec<char>>;
}

fn int_arg(args: &impl Args, i: usize) -> Result<i64> {
    match args.int(i) {
        Some(n) => Ok(n),
        None => bail!("expected an integer argument"),
    }
}

/// An argument the script's own types constrain to usize, so a negative or
/// oversized value can only come from an interpreter bug or an invalid
/// program, and errors instead of wrapping.
fn usize_arg(args: &impl Args, i: usize) -> Result<usize> {
    let n = int_arg(args, i)?;
    usize::try_from(n).map_err(|_| anyhow!("`{n}` is not a valid count"))
}

/// A length or byte offset as the integer scripts see. These fit i64 on every
/// supported platform, the expect documents the impossible case.
pub(super) fn usize_i64(i: usize) -> i64 {
    i64::try_from(i).expect("value exceeds i64")
}

/// A length as a real `usize` value, width tag included. An untagged length
/// ran `!` and underflow in i64, so `!v.len()` answered a small negative
/// where compiled Rust answers a huge unsigned. Found by the differential
/// campaign at seed 20675317577.
pub(super) fn usize_value(i: usize) -> super::value::Value {
    super::value::Value::int_of_width(i128::from(usize_i64(i)), IntWidth::USize)
}

fn float_arg(args: &impl Args, i: usize) -> Result<f64> {
    match args.float(i) {
        Some(f) => Ok(f),
        None => bail!("expected a float argument"),
    }
}

// -- numbers ---------------------------------------------------------------

#[derive(Clone, Copy)]
pub(super) enum Num {
    Int(i64),
    Float(f64),
}

/// What a numeric method produced, materialized into a value by the caller.
pub(super) enum NumOut {
    Int(i64),
    Float(f64),
    Bool(bool),
    SomeInt(i64),
    SomeFloat(f64),
    Nothing,
    Ordering(Ordering),
    SomeOrdering(Ordering),
}

pub(super) fn num_core(recv: Num, name: BuiltinId, args: &impl Args) -> Result<Option<NumOut>> {
    use Num::{Float, Int};
    use NumOut as O;
    let as_f = || match recv {
        Int(i) => AsPrimitive::<f64>::as_(i),
        Float(f) => f,
    };
    Ok(Some(match (recv, name) {
        // `as_i64`, `as_u64` and `as_f64` on an integer are range checked in
        // `int_methods`, which sees the real width, and never reach here.
        (Int(i), B::AsI128 | B::AsUsize) => O::SomeInt(i),
        // serde_json keeps every json float as f64 and its integer accessors
        // answer None on it, even for a whole value like 5.0.
        (Float(_), B::AsI64 | B::AsU64 | B::AsI128 | B::AsUsize) => O::Nothing,
        (Float(f), B::AsF64) => O::SomeFloat(f),
        // A number is not these serde types, so the accessor is None.
        (_, B::AsStr | B::AsBool | B::AsArray | B::AsArrayMut | B::AsObject | B::AsObjectMut) => {
            O::Nothing
        }
        (Int(i), B::Abs) => O::Int(i.abs()),
        (Float(f), B::Abs) => O::Float(f.abs()),
        (Int(i), B::Pow) => O::Int(i.pow(u32::try_from(int_arg(args, 0)?)?)),
        (Float(f), B::Powi) => O::Float(f.powi(i32::try_from(int_arg(args, 0)?)?)),
        (Float(f), B::Powf) => O::Float(f.powf(float_arg(args, 0)?)),
        (Float(f), B::Sqrt) => O::Float(f.sqrt()),
        (Float(f), B::Floor) => O::Float(f.floor()),
        (Float(f), B::Trunc) => O::Float(f.trunc()),
        // Float methods on an int receiver: the untyped `parse` guesses a
        // whole float like "160" into an int, and the annotation that made it
        // f64 in real Rust is erased at runtime. Rounding is identity there,
        // and the rest compute through the float view.
        (Int(i), B::Trunc | B::Floor | B::Ceil | B::Round) => O::Int(i),
        (Int(_), B::Sqrt) => O::Float(as_f().sqrt()),
        (Int(_), B::Powi) => O::Float(as_f().powi(i32::try_from(int_arg(args, 0)?)?)),
        (Int(_), B::Powf) => O::Float(as_f().powf(float_arg(args, 0)?)),
        (Int(i), B::IsSignPositive) => O::Bool(i >= 0),
        (Float(f), B::Ceil) => O::Float(f.ceil()),
        (Float(f), B::Round) => O::Float(f.round()),
        (Float(f), B::IsSignPositive) => O::Bool(f.is_sign_positive()),
        (Float(f), B::Fract) => O::Float(f.fract()),
        // A whole value parse-guessed into an int has no fraction.
        (Int(_), B::Fract) => O::Int(0),
        // Int signum answers width-aware in `int_methods`, only the float
        // side lives here.
        (Float(f), B::Signum) => O::Float(f.signum()),
        (Float(f), B::Recip) => O::Float(f.recip()),
        (Int(_), B::Recip) => O::Float(as_f().recip()),
        (Float(f), B::MulAdd) => O::Float(f.mul_add(float_arg(args, 0)?, float_arg(args, 1)?)),
        (Int(_), B::MulAdd) => O::Float(as_f().mul_add(float_arg(args, 0)?, float_arg(args, 1)?)),
        (Float(f), B::IsNan) => O::Bool(f.is_nan()),
        (Float(f), B::IsFinite) => O::Bool(f.is_finite()),
        (Int(_), B::IsFinite) => O::Bool(true),
        (Float(f), B::IsInfinite) => O::Bool(f.is_infinite()),
        (Int(_), B::IsNan | B::IsInfinite) => O::Bool(false),
        (Float(f), B::IsSignNegative) => O::Bool(f.is_sign_negative()),
        (Int(i), B::IsSignNegative) => O::Bool(i < 0),
        (Int(a), B::Min) => O::Int(a.min(int_arg(args, 0)?)),
        (Int(a), B::Max) => O::Int(a.max(int_arg(args, 0)?)),
        (Int(a), B::Clamp) => O::Int(a.clamp(int_arg(args, 0)?, int_arg(args, 1)?)),
        (Float(a), B::Clamp) => O::Float(a.clamp(float_arg(args, 0)?, float_arg(args, 1)?)),
        (Float(a), B::Min) => O::Float(a.min(float_arg(args, 0)?)),
        (Float(a), B::Max) => O::Float(a.max(float_arg(args, 0)?)),
        (Int(a), B::IsMultipleOf) => O::Bool(a % int_arg(args, 0)? == 0),
        (Int(a), B::SaturatingSub) => O::Int(a.saturating_sub(int_arg(args, 0)?)),
        (Int(a), B::SaturatingAdd) => O::Int(a.saturating_add(int_arg(args, 0)?)),
        (Int(a), B::SaturatingMul) => O::Int(a.saturating_mul(int_arg(args, 0)?)),
        (Int(a), B::Cmp) => O::Ordering(a.cmp(&int_arg(args, 0)?)),
        (_, B::PartialCmp) => O::SomeOrdering(
            as_f()
                .partial_cmp(&float_arg(args, 0)?)
                .unwrap_or(Ordering::Equal),
        ),
        _ => return Ok(None),
    }))
}

/// What an f32 method produced, materialized into a value by the caller.
pub(super) enum F32Out {
    Val(f32),
    Bool(bool),
    SomeOrdering(Ordering),
}

/// The f32 method surface, computed in real f32 so results match a compiled
/// binary bit for bit. Routing an f32 through the f64 core double rounds
/// `sqrt` and friends, and the result forgets it was an f32, so `{:?}` printed
/// the f64 shortest form, `3.4028234663852886e38` instead of `3.4028235e38`
/// for `f32::MAX`.
pub(super) fn f32_core(recv: f32, name: BuiltinId, args: &impl Args) -> Result<Option<F32Out>> {
    use F32Out as O;
    let arg = |i: usize| -> Result<f32> { float_arg(args, i).map(AsPrimitive::<f32>::as_) };
    Ok(Some(match name {
        B::Abs => O::Val(recv.abs()),
        B::Powi => O::Val(recv.powi(i32::try_from(int_arg(args, 0)?)?)),
        B::Powf => O::Val(recv.powf(arg(0)?)),
        B::Sqrt => O::Val(recv.sqrt()),
        B::Floor => O::Val(recv.floor()),
        B::Trunc => O::Val(recv.trunc()),
        B::Ceil => O::Val(recv.ceil()),
        B::Round => O::Val(recv.round()),
        B::Min => O::Val(recv.min(arg(0)?)),
        B::Max => O::Val(recv.max(arg(0)?)),
        B::Clamp => O::Val(recv.clamp(arg(0)?, arg(1)?)),
        B::Fract => O::Val(recv.fract()),
        B::Signum => O::Val(recv.signum()),
        B::Recip => O::Val(recv.recip()),
        B::MulAdd => O::Val(recv.mul_add(arg(0)?, arg(1)?)),
        B::IsSignPositive => O::Bool(recv.is_sign_positive()),
        B::IsSignNegative => O::Bool(recv.is_sign_negative()),
        B::IsNan => O::Bool(recv.is_nan()),
        B::IsFinite => O::Bool(recv.is_finite()),
        B::IsInfinite => O::Bool(recv.is_infinite()),
        // The same answer the f64 core gives, so both precisions stay in step.
        B::PartialCmp => O::SomeOrdering(recv.partial_cmp(&arg(0)?).unwrap_or(Ordering::Equal)),
        _ => return Ok(None),
    }))
}

// -- json ------------------------------------------------------------------

/// The shape a decoded json value has once it is an interpreter value. A
/// parsed json is held as plain values, an object as a map and a string as a
/// string, so the serde type tests are shape tests. The dispatch layer maps a
/// value onto this and answers from one table.
#[derive(Clone, Copy)]
pub(super) enum JsonKind {
    Object,
    Array,
    Str,
    Bool,
    /// An integer at its real value, so the range tests can answer on it.
    Int(i128),
    Float,
    Null,
    Other,
}

/// The `serde_json` `is_*` family. These apply to every receiver, so they are
/// answered before the per type dispatch, which returns early for the hot
/// receivers and would otherwise never reach them.
pub(super) fn json_type_test(kind: JsonKind, name: BuiltinId) -> Option<bool> {
    use JsonKind as K;
    Some(match name {
        B::IsObject => matches!(kind, K::Object),
        B::IsArray => matches!(kind, K::Array),
        B::IsString => matches!(kind, K::Str),
        B::IsBoolean => matches!(kind, K::Bool),
        B::IsNumber => matches!(kind, K::Int(_) | K::Float),
        // serde answers these by range, not by "it is an integer". A negative
        // number is not a u64, and one past `i64::MAX` is not an i64.
        B::IsI64 => matches!(kind, K::Int(v) if i64::try_from(v).is_ok()),
        B::IsU64 => matches!(kind, K::Int(v) if u64::try_from(v).is_ok()),
        B::IsF64 => matches!(kind, K::Float),
        B::IsNull => matches!(kind, K::Null),
        _ => return None,
    })
}

/// The `serde_json` `as_*` family, by name only. A receiver of the wrong shape
/// answers None rather than erroring, so the caller tests the name here and
/// then decides whether the receiver matches.
pub(super) fn json_accessor(name: BuiltinId) -> bool {
    matches!(
        name,
        B::AsStr
            | B::AsI64
            | B::AsU64
            | B::AsF64
            | B::AsBool
            | B::AsArray
            | B::AsArrayMut
            | B::AsObject
            | B::AsObjectMut
    )
}

/// The tokens of a json pointer, RFC 6901, or None when the text is not a
/// pointer at all. An empty pointer selects the whole value, so it yields no
/// tokens. `~1` and `~0` are the escapes for a slash and a tilde in a key.
pub(super) fn json_pointer_tokens(pointer: &str) -> Option<Vec<String>> {
    if pointer.is_empty() {
        return Some(Vec::new());
    }
    if !pointer.starts_with('/') {
        return None;
    }
    Some(
        pointer
            .split('/')
            .skip(1)
            .map(|token| token.replace("~1", "/").replace("~0", "~"))
            .collect(),
    )
}

/// A pointer token as an array index. serde rejects a leading plus and a
/// leading zero, so "01" is not index one.
pub(super) fn json_pointer_index(token: &str) -> Option<usize> {
    if token.starts_with('+') || (token.starts_with('0') && token.len() != 1) {
        return None;
    }
    token.parse().ok()
}

// -- chars -----------------------------------------------------------------

/// The result of a `char` method, in a form the caller turns into a value.
/// Keeps the classification table in one place.
pub(super) enum CharOut {
    Bool(bool),
    Char(char),
    Str(String),
    /// `to_digit`, whose payload is a u32 in real Rust.
    OptU32(Option<u32>),
}

/// The `char` classification and conversion methods.
pub(super) fn char_method(ch: char, name: BuiltinId, args: &impl Args) -> Option<Result<CharOut>> {
    let b = |v: bool| Some(Ok(CharOut::Bool(v)));
    match name {
        B::ToDigit => {
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
        B::IsAsciiDigit => b(ch.is_ascii_digit()),
        B::IsAsciiAlphabetic => b(ch.is_ascii_alphabetic()),
        B::IsAsciiAlphanumeric => b(ch.is_ascii_alphanumeric()),
        B::IsAsciiUppercase => b(ch.is_ascii_uppercase()),
        B::IsAsciiLowercase => b(ch.is_ascii_lowercase()),
        B::IsAsciiWhitespace => b(ch.is_ascii_whitespace()),
        B::IsAsciiPunctuation => b(ch.is_ascii_punctuation()),
        B::IsAsciiHexdigit => b(ch.is_ascii_hexdigit()),
        B::IsAscii => b(ch.is_ascii()),
        B::IsAlphabetic => b(ch.is_alphabetic()),
        B::IsAlphanumeric => b(ch.is_alphanumeric()),
        B::IsNumeric => b(ch.is_numeric()),
        B::IsWhitespace => b(ch.is_whitespace()),
        B::IsUppercase => b(ch.is_uppercase()),
        B::IsLowercase => b(ch.is_lowercase()),
        B::ToAsciiUppercase => Some(Ok(CharOut::Char(ch.to_ascii_uppercase()))),
        B::ToAsciiLowercase => Some(Ok(CharOut::Char(ch.to_ascii_lowercase()))),
        // These yield an iterator in real Rust, but a script only ever renders
        // or collects it, so the string it would produce is handed back.
        B::ToUppercase => Some(Ok(CharOut::Str(ch.to_uppercase().to_string()))),
        B::ToLowercase => Some(Ok(CharOut::Str(ch.to_lowercase().to_string()))),
        _ => None,
    }
}

// -- strings ---------------------------------------------------------------

/// What a string method produced, materialized by the caller. `Keep` and
/// `OkKeep` hand the receiver back so the caller answers with a refcount
/// bump, never a copy.
pub(super) enum StrOut {
    Bool(bool),
    /// A length or count, materialized with the real `usize` width.
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

/// The untyped `parse` guess: int first, then float, then bool.
pub(super) fn str_core(s: &str, name: BuiltinId, args: &impl Args) -> Result<Option<StrOut>> {
    use StrOut as O;
    let a = |i: usize| args.text(i);
    Ok(Some(match name {
        B::Len => O::USize(s.len()),
        B::IsEmpty => O::Bool(s.is_empty()),
        B::Count => O::USize(s.chars().count()),
        B::Contains => O::Bool(s.contains(&a(0))),
        B::EqIgnoreAsciiCase => O::Bool(s.eq_ignore_ascii_case(&a(0))),
        B::StartsWith => O::Bool(s.starts_with(&a(0))),
        B::EndsWith => O::Bool(s.ends_with(&a(0))),
        B::Trim => O::Owned(s.trim().to_string()),
        B::TrimStart => O::Owned(s.trim_start().to_string()),
        B::TrimEnd => O::Owned(s.trim_end().to_string()),
        B::ToUppercase => O::Owned(s.to_uppercase()),
        B::ToLowercase => O::Owned(s.to_lowercase()),
        // The ascii variants leave non-ascii characters alone, they are not
        // aliases of the unicode ones.
        B::ToAsciiUppercase => O::Owned(s.to_ascii_uppercase()),
        B::ToAsciiLowercase => O::Owned(s.to_ascii_lowercase()),
        // A char-set pattern like `[':', '.']` replaces any of its members, matching real Rust. Without
        // this the array renders as text and matches nothing, silently leaving the string unchanged.
        B::Replace => match args.pattern_chars(0) {
            Some(cs) => O::Owned(s.replace(cs.as_slice(), &a(1))),
            None => O::Owned(s.replace(&a(0), &a(1))),
        },
        B::Replacen => match args.pattern_chars(0) {
            Some(cs) => O::Owned(s.replacen(cs.as_slice(), &a(1), usize_arg(args, 2)?)),
            None => O::Owned(s.replacen(&a(0), &a(1), usize_arg(args, 2)?)),
        },
        B::Repeat => {
            let n = args
                .int(0)
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(0);
            O::Owned(s.repeat(n))
        }
        // String::as_str gives the string back. serde_json::Value::as_str
        // gives an Option, and a json string is a plain Str here, so unwrap
        // and expect on a string are identity to keep serde chains working.
        // A String or a Cow that already owns its data, into_owned is self.
        B::ToOwned
        | B::TrimString
        | B::AsStr
        | B::AsString
        | B::Unwrap
        | B::Expect
        | B::UnwrapOr
        | B::UnwrapOrElse
        | B::UnwrapOrDefault
        | B::IntoOwned
        | B::IntoString => O::Keep,
        // `Option::context` returns a Result, so the pre-unwrapped string has
        // to come back wrapped or a following `?` would have nothing to unwrap.
        B::Context | B::WithContext => O::OkKeep,
        B::IsSome => O::Bool(true),
        B::IsNone => O::Bool(false),
        B::AsBytes | B::IntoBytes => O::Ints(s.bytes().map(i64::from).collect()),
        // The utf-16 code units as an eager list of ints, mirroring `bytes`.
        B::EncodeUtf16 => O::Ints(s.encode_utf16().map(i64::from).collect()),
        B::StripPrefix => O::OptOwned(s.strip_prefix(&a(0)).map(str::to_string)),
        B::StripSuffix => O::OptOwned(s.strip_suffix(&a(0)).map(str::to_string)),
        // Byte offsets, same as the real std, and slicing is byte based too,
        // so `&s[..s.find(x).unwrap()]` behaves right.
        B::Find => O::OptInt(s.find(&a(0)).map(usize_i64)),
        B::Rfind => O::OptInt(s.rfind(&a(0)).map(usize_i64)),
        B::SplitOnce => O::OptPair(
            s.split_once(&a(0))
                .map(|(x, y)| (x.to_string(), y.to_string())),
        ),
        B::RsplitOnce => O::OptPair(
            s.rsplit_once(&a(0))
                .map(|(x, y)| (x.to_string(), y.to_string())),
        ),
        // A char array like `['-', '_']` splits on any of its members, which
        // a plain string pattern would only match as the literal sequence.
        B::Split => match args.pattern_chars(0) {
            Some(chars) => O::Strs(
                s.split(|c: char| chars.contains(&c))
                    .map(str::to_string)
                    .collect(),
            ),
            None => O::Strs(s.split(&a(0)).map(str::to_string).collect()),
        },
        B::Rsplit => O::Strs(s.rsplit(&a(0)).map(str::to_string).collect()),
        B::Splitn => {
            let n = usize_arg(args, 0)?;
            O::Strs(s.splitn(n, &a(1)).map(str::to_string).collect())
        }
        B::Rsplitn => {
            let n = usize_arg(args, 0)?;
            O::Strs(s.rsplitn(n, &a(1)).map(str::to_string).collect())
        }
        B::Matches => O::Strs(s.matches(&a(0)).map(str::to_string).collect()),
        B::CharIndices => O::CharIdx(s.char_indices().map(|(i, c)| (usize_i64(i), c)).collect()),
        B::TrimMatches | B::TrimStartMatches | B::TrimEndMatches => {
            let pat = a(0);
            let out = match name {
                B::TrimStartMatches => s.trim_start_matches(&pat),
                B::TrimEndMatches => s.trim_end_matches(&pat),
                // trim_matches only takes chars in real Rust.
                _ => match args.pattern_chars(0) {
                    Some(chars) => s.trim_matches(|c: char| chars.contains(&c)),
                    None => s.trim_matches(pat.chars().next().unwrap_or(' ')),
                },
            };
            O::Owned(out.to_string())
        }
        B::Cmp => O::Ordering(s.cmp(a(0).as_str())),
        // `parse` without a turbofish is answered through `parse_core`, which
        // is the only place that sees the target type.
        _ => return Ok(None),
    }))
}

/// What `str::parse` produced, before the caller wraps it in an `Ok`.
pub(super) enum Parsed {
    Int(i128, IntWidth),
    F32(f32),
    F64(f64),
    Bool(bool),
    Char(char),
    Str(String),
    Fail(String),
}

/// The two `ParseIntError` range messages, written out because std exposes no
/// way to build that error from outside the standard library. Every other parse
/// failure carries the real error, so only these two are mirrored by hand.
fn out_of_range(too_small: bool) -> String {
    if too_small {
        "number too small to fit in target type".to_string()
    } else {
        "number too large to fit in target type".to_string()
    }
}

/// What a `text.parse::<i64>()` would report for text that is not a number.
fn int_error(text: &str) -> String {
    text.parse::<i64>()
        .err()
        .map_or_else(|| format!("cannot parse `{text}`"), |e| e.to_string())
}

/// `str::parse`, honoring the target type when the call wrote one down.
///
/// Real Rust decides this entirely by the target: the text must be the whole
/// value with no surrounding whitespace, and an integer target rejects
/// anything outside its own range. Guessing instead made `"300".parse::<u8>()`
/// an `Ok(300)` and `" 5 ".parse::<i64>()` an `Ok(5)`, both of which real Rust
/// rejects. Without a turbofish there is no type to honor, so the old guess
/// stays, which is what a plain `let n: u8 = s.parse()?` still lands on.
pub(super) fn parse_core(text: &str, target: Option<&ScalarTy>) -> Parsed {
    let fail = |e: &dyn std::fmt::Display| Parsed::Fail(e.to_string());
    let Some(target) = target else {
        let trimmed = text.trim();
        return if let Ok(value) = trimmed.parse::<i64>() {
            Parsed::Int(i128::from(value), IntWidth::I64)
        } else if let Ok(value) = trimmed.parse::<u128>() {
            // An integer past i64 keeps its exact digits at 128 bits, a
            // float fallback would round them away.
            Parsed::Int(value.cast_signed(), IntWidth::U128)
        } else if let Ok(value) = trimmed.parse::<i128>() {
            Parsed::Int(value, IntWidth::I128)
        } else if let Ok(value) = trimmed.parse::<f64>() {
            Parsed::F64(value)
        } else if let Ok(value) = trimmed.parse::<bool>() {
            Parsed::Bool(value)
        } else {
            // All three failed, so report what an integer parse would have
            // said. That is the common intent and it is a real std message.
            Parsed::Fail(int_error(trimmed))
        };
    };
    match target {
        // An unsigned target rejects a minus sign as an invalid digit before
        // any range check, so `"-0"` is an error even though its value fits.
        // Parsing through i128 accepted it by range, u128 refuses the sign
        // with the real std message.
        ScalarTy::Int(IntWidth::U128) => match text.parse::<u128>() {
            Ok(value) => Parsed::Int(value.cast_signed(), IntWidth::U128),
            Err(e) => fail(&e),
        },
        ScalarTy::Int(width) if !width.is_signed() => match text.parse::<u128>() {
            Ok(value) => match i128::try_from(value) {
                Ok(value) if value <= width.max() => Parsed::Int(value, *width),
                _ => Parsed::Fail(out_of_range(false)),
            },
            Err(e) => fail(&e),
        },
        ScalarTy::Int(width) => match text.parse::<i128>() {
            Ok(value) if value >= width.min() && value <= width.max() => Parsed::Int(value, *width),
            Ok(value) => Parsed::Fail(out_of_range(value < width.min())),
            // i128 is wider than every target, so a failure here is the text
            // being unparseable rather than the target being too narrow, and
            // the message reads the same for any width.
            Err(e) => fail(&e),
        },
        ScalarTy::F32 => text.parse::<f32>().map_or_else(|e| fail(&e), Parsed::F32),
        ScalarTy::F64 => text.parse::<f64>().map_or_else(|e| fail(&e), Parsed::F64),
        ScalarTy::Bool => text.parse::<bool>().map_or_else(|e| fail(&e), Parsed::Bool),
        ScalarTy::Char => text.parse::<char>().map_or_else(|e| fail(&e), Parsed::Char),
        ScalarTy::Str => Parsed::Str(text.to_string()),
        // No container implements `FromStr`, so these never name a parse
        // target. They exist only to describe a `Default`.
        ScalarTy::Opt(_)
        | ScalarTy::List(_)
        | ScalarTy::Map(_)
        | ScalarTy::Set(_)
        | ScalarTy::Other => Parsed::Fail(format!("cannot parse `{text}`")),
    }
}

// -- regex -----------------------------------------------------------------

/// What a `Regex` method produced. Spans index into the source string the
/// caller already holds, so the caller materializes the match handles.
pub(super) enum RegexOut {
    Bool(bool),
    Text(String),
    /// Answered with the shared pattern handle.
    Pattern,
    /// `find`: the first match's span, if any.
    OptSpan(Option<(usize, usize)>),
    /// `captures`: per group, its span when the group matched.
    OptGroups(Option<Vec<Option<(usize, usize)>>>),
    /// `split`: the pieces as owned strings.
    Pieces(Vec<String>),
}

/// The eager `Regex` methods. The `find_iter` and `captures_iter` forms stay
/// out of the core, the interpreter streams them lazily and
/// collects them.
pub(super) fn regex_core(
    re: &regex::Regex,
    name: BuiltinId,
    source: &str,
    replacement: &dyn Fn() -> String,
) -> Option<RegexOut> {
    use RegexOut as O;
    Some(match name {
        B::IsMatch => O::Bool(re.is_match(source)),
        B::Find => O::OptSpan(re.find(source).map(|m| (m.start(), m.end()))),
        B::Captures => O::OptGroups(re.captures(source).map(|c| {
            (0..c.len())
                .map(|i| c.get(i).map(|g| (g.start(), g.end())))
                .collect()
        })),
        B::Replace => O::Text(re.replacen(source, 1, replacement().as_str()).into_owned()),
        B::ReplaceAll => O::Text(re.replace_all(source, replacement().as_str()).into_owned()),
        B::Split => O::Pieces(re.split(source).map(str::to_string).collect()),
        B::AsStr => O::Pattern,
        _ => return None,
    })
}

/// A `Match` method over its span.
pub(super) enum MatchOut {
    Text(String),
    Int(i64),
}

pub(super) fn match_core(
    name: BuiltinId,
    source: &str,
    start: usize,
    end: usize,
) -> Option<MatchOut> {
    Some(match name {
        B::AsStr => MatchOut::Text(source[start..end].to_string()),
        B::Start => MatchOut::Int(usize_i64(start)),
        B::End => MatchOut::Int(usize_i64(end)),
        _ => return None,
    })
}

/// A `Captures` method: a group lookup resolved to its span, or the count.
pub(super) enum CapturesOut {
    Int(i64),
    /// The queried group's span, None when absent, out of range, or unmatched.
    OptSpan(Option<(usize, usize)>),
}

pub(super) fn captures_core<'n>(
    name: BuiltinId,
    groups: &[Option<(usize, usize)>],
    mut names: impl Iterator<Item = (&'n str, usize)>,
    args: &impl Args,
) -> Result<Option<CapturesOut>> {
    use CapturesOut as O;
    Ok(Some(match name {
        B::Get => {
            let Some(index) = args.int(0).and_then(|i| usize::try_from(i).ok()) else {
                bail!("captures get needs a non-negative index");
            };
            O::OptSpan(groups.get(index).copied().flatten())
        }
        B::Name => {
            let wanted = args.text(0);
            let index = names.find_map(|(n, i)| (n == wanted).then_some(i));
            O::OptSpan(index.and_then(|i| groups.get(i).copied().flatten()))
        }
        B::Len => O::Int(usize_i64(groups.len())),
        _ => return Ok(None),
    }))
}

// -- duration --------------------------------------------------------------

pub(super) enum DurOut {
    Int(i64),
    Float(f64),
    Bool(bool),
}

/// `Duration + Duration` and `Duration - Duration`, the checked std ops with
/// the real panic messages as the errors. Any other operator on two durations
/// does not exist in std either.
pub(super) fn duration_arith(op: BinKind, a: Duration, b: Duration) -> Result<Duration> {
    match op {
        BinKind::Add => a
            .checked_add(b)
            .ok_or_else(|| anyhow!("overflow when adding durations")),
        BinKind::Sub => a
            .checked_sub(b)
            .ok_or_else(|| anyhow!("overflow when subtracting durations")),
        _ => bail!("cannot apply that operator to two durations"),
    }
}

/// `Duration` accessors over the real `secs` plus `nanos` split, exactly the
/// std methods per name.
pub(super) fn duration_core(name: BuiltinId, secs: u64, nanos: u32) -> Option<DurOut> {
    use DurOut as O;
    let total = u128::from(secs) * 1_000_000_000 + u128::from(nanos);
    Some(match name {
        B::AsSecs => O::Int(i64::try_from(secs).unwrap_or(i64::MAX)),
        B::AsMillis => O::Int(i64::try_from(total / 1_000_000).unwrap_or(i64::MAX)),
        B::AsMicros => O::Int(i64::try_from(total / 1_000).unwrap_or(i64::MAX)),
        B::AsNanos => O::Int(i64::try_from(total).unwrap_or(i64::MAX)),
        B::SubsecNanos => O::Int(i64::from(nanos)),
        B::SubsecMillis => O::Int(i64::from(nanos / 1_000_000)),
        B::SubsecMicros => O::Int(i64::from(nanos / 1_000)),
        B::AsSecsF64 => O::Float(AsPrimitive::<f64>::as_(secs) + f64::from(nanos) / 1e9),
        B::IsZero => O::Bool(total == 0),
        _ => return None,
    })
}

// -- datetime ---------------------------------------------------------------

pub(super) enum DateOut {
    Int(i64),
    Text(String),
}

/// `DateTime::parse_from_rfc3339` reduced to the three numbers the bridge
/// store for a datetime, the unix seconds, the sub second nanos, and the
/// seconds east of UTC the text carried. The error is the real chrono
/// `ParseError` rendered as text, so a script sees the same message it would
/// get compiled.
pub(super) fn parse_rfc3339(text: &str) -> Result<(i64, u32, i32), String> {
    use chrono::{DateTime, Offset, Timelike};
    match DateTime::parse_from_rfc3339(text) {
        Ok(dt) => Ok((
            dt.timestamp(),
            dt.nanosecond(),
            dt.offset().fix().local_minus_utc(),
        )),
        Err(e) => Err(e.to_string()),
    }
}

/// `DateTime` accessors over the stored unix timestamp. `local` selects the
/// machine timezone, otherwise the value is read through `offset`, the seconds
/// east of UTC that a parsed timestamp carried. `Utc::now` stores a zero
/// offset, so it still reads as UTC. Every accessor goes through one fixed
/// offset view, which is what real chrono does, a calendar field is read in the
/// zone the value carries and not in UTC.
pub(super) fn datetime_core(
    name: BuiltinId,
    secs: i64,
    nanos: u32,
    local: bool,
    offset: i32,
    args: &impl Args,
) -> Option<DateOut> {
    use DateOut as O;
    use chrono::{DateTime, Datelike, FixedOffset, Local, Offset, Timelike, Utc};
    let utc: DateTime<Utc> = DateTime::from_timestamp(secs, nanos).unwrap_or_default();
    let view = if local {
        utc.with_timezone(&Local).fixed_offset()
    } else {
        utc.with_timezone(&FixedOffset::east_opt(offset).unwrap_or(Utc.fix()))
    };
    Some(match name {
        B::Timestamp => O::Int(secs),
        B::TimestampMillis => O::Int(secs * 1000 + i64::from(nanos / 1_000_000)),
        B::ToRfc3339 => O::Text(view.to_rfc3339()),
        B::Format => O::Text(view.format(&args.text(0)).to_string()),
        B::Year => O::Int(i64::from(view.year())),
        B::Month => O::Int(i64::from(view.month())),
        B::Day => O::Int(i64::from(view.day())),
        B::Hour => O::Int(i64::from(view.hour())),
        B::Minute => O::Int(i64::from(view.minute())),
        B::Second => O::Int(i64::from(view.second())),
        _ => return None,
    })
}

// -- http and process scalars ----------------------------------------------

/// `StatusCode` accessors over the numeric code.
pub(super) enum StatusOut {
    Int(i64),
    Bool(bool),
}

pub(super) fn status_core(name: BuiltinId, code: i64) -> Option<StatusOut> {
    use StatusOut as O;
    Some(match name {
        B::AsU16 | B::AsInt => O::Int(code),
        B::IsSuccess => O::Bool((200..300).contains(&code)),
        B::IsClientError => O::Bool((400..500).contains(&code)),
        B::IsServerError => O::Bool((500..600).contains(&code)),
        _ => return None,
    })
}

/// `HeaderValue` accessors over the header's text.
pub(super) enum HeaderOut {
    /// `to_str` answers `Ok(text)`, like the real fallible accessor.
    Ok(String),
    Text(String),
}

pub(super) fn header_value_core(name: BuiltinId, text: String) -> Option<HeaderOut> {
    Some(match name {
        B::ToStr => HeaderOut::Ok(text),
        B::AsStr | B::AsString | B::ToString => HeaderOut::Text(text),
        _ => return None,
    })
}

/// `ExitStatus` accessors over the flag and the optional code.
pub(super) enum ExitOut {
    Bool(bool),
    /// `code()`: `Some(code)` normally, `None` after death by signal.
    OptInt(Option<i64>),
}

pub(super) fn exit_status_core(
    name: BuiltinId,
    success: bool,
    code: Option<i64>,
) -> Option<ExitOut> {
    Some(match name {
        B::Success => ExitOut::Bool(success),
        B::Code => ExitOut::OptInt(code),
        _ => return None,
    })
}

/// The `colored` crate as string methods, shared so tokio scripts color their
/// output the same way. Returns the styled text as a plain string carrying
/// ANSI codes, so chaining and printing both work. Honors the crate's own
/// `NO_COLOR` and terminal detection.
pub(super) fn color_core(s: &str, name: BuiltinId) -> Option<String> {
    use colored::Colorize;
    let out = match name {
        B::Red => s.red(),
        B::Green => s.green(),
        B::Yellow => s.yellow(),
        B::Blue => s.blue(),
        B::Magenta | B::Purple => s.magenta(),
        B::Cyan => s.cyan(),
        B::White => s.white(),
        B::Black => s.black(),
        B::BrightRed => s.bright_red(),
        B::BrightGreen => s.bright_green(),
        B::BrightYellow => s.bright_yellow(),
        B::BrightBlue => s.bright_blue(),
        B::BrightCyan => s.bright_cyan(),
        B::OnRed => s.on_red(),
        B::OnGreen => s.on_green(),
        B::OnBlue => s.on_blue(),
        B::Bold => s.bold(),
        B::Dimmed => s.dimmed(),
        B::Italic => s.italic(),
        B::Underline => s.underline(),
        B::Reversed => s.reversed(),
        B::Clear | B::Normal => s.normal(),
        _ => return None,
    };
    Some(out.to_string())
}
