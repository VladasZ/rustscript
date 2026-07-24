//! Engine neutral method cores, written once and materialized by both engines.
//!
//! The fast engine and the parallel engine used to carry their own copy of
//! every scalar method, and the copies drifted. A core here works on plain
//! Rust types and answers through a small output enum, so each engine only
//! adapts arguments in and values out. The coverage harvest reads this file
//! once as `Engine::Both`, so a method added here reaches both engines and
//! both tables in the same commit.
//!
//! What stays engine side: anything lazy or stateful. The fast engine's
//! iterator forms of `chars`, `lines`, `bytes`, and `split_whitespace` cannot
//! be expressed as a finished value, and containers live behind different
//! cell types per engine.

use std::cmp::Ordering;

use anyhow::{Result, bail};

/// Engine neutral view of a method's arguments. Each engine adapts its own
/// value slice; the cores monomorphize over this, so the view costs nothing.
pub(super) trait Args {
    /// The argument rendered as text, what `Display` would print. Missing
    /// arguments render empty, matching how both engines behaved.
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

/// What a numeric method produced, materialized by each engine.
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

pub(super) fn num_core(recv: Num, name: &str, args: &impl Args) -> Result<Option<NumOut>> {
    use Num::{Float, Int};
    use NumOut as O;
    let as_f = || match recv {
        Int(i) => i as f64,
        Float(f) => f,
    };
    Ok(Some(match (recv, name) {
        (Int(i), "as_i64" | "as_u64" | "as_i128" | "as_usize") => O::SomeInt(i),
        // serde_json keeps every json float as f64 and its integer accessors
        // answer None on it, even for a whole value like 5.0.
        (Float(_), "as_i64" | "as_u64" | "as_i128" | "as_usize") => O::Nothing,
        (_, "as_f64") => O::SomeFloat(as_f()),
        // A number is not these serde types, so the accessor is None.
        (_, "as_str" | "as_bool" | "as_array" | "as_object") => O::Nothing,
        (Int(i), "abs") => O::Int(i.abs()),
        (Float(f), "abs") => O::Float(f.abs()),
        (Int(i), "pow") => O::Int(i.pow(int_arg(args, 0)? as u32)),
        (Float(f), "powi") => O::Float(f.powi(int_arg(args, 0)? as i32)),
        (Float(f), "powf") => O::Float(f.powf(float_arg(args, 0)?)),
        (Float(f), "sqrt") => O::Float(f.sqrt()),
        (Float(f), "floor") => O::Float(f.floor()),
        (Float(f), "trunc") => O::Float(f.trunc()),
        // Float methods on an int receiver: the untyped `parse` guesses a
        // whole float like "160" into an int, and the annotation that made it
        // f64 in real Rust is erased at runtime. Rounding is identity there,
        // and the rest compute through the float view.
        (Int(i), "trunc" | "floor" | "ceil" | "round") => O::Int(i),
        (Int(_), "sqrt") => O::Float(as_f().sqrt()),
        (Int(_), "powi") => O::Float(as_f().powi(int_arg(args, 0)? as i32)),
        (Int(_), "powf") => O::Float(as_f().powf(float_arg(args, 0)?)),
        (Int(i), "is_sign_positive") => O::Bool(i >= 0),
        (Float(f), "ceil") => O::Float(f.ceil()),
        (Float(f), "round") => O::Float(f.round()),
        (Float(f), "is_sign_positive") => O::Bool(f.is_sign_positive()),
        (Int(a), "min") => O::Int(a.min(int_arg(args, 0)?)),
        (Int(a), "max") => O::Int(a.max(int_arg(args, 0)?)),
        (Int(a), "clamp") => O::Int(a.clamp(int_arg(args, 0)?, int_arg(args, 1)?)),
        (Float(a), "clamp") => O::Float(a.clamp(float_arg(args, 0)?, float_arg(args, 1)?)),
        (Float(a), "min") => O::Float(a.min(float_arg(args, 0)?)),
        (Float(a), "max") => O::Float(a.max(float_arg(args, 0)?)),
        (Int(a), "is_multiple_of") => O::Bool(a % int_arg(args, 0)? == 0),
        (Int(a), "saturating_sub") => O::Int(a.saturating_sub(int_arg(args, 0)?)),
        (Int(a), "saturating_add") => O::Int(a.saturating_add(int_arg(args, 0)?)),
        (Int(a), "saturating_mul") => O::Int(a.saturating_mul(int_arg(args, 0)?)),
        (Int(a), "cmp") => O::Ordering(a.cmp(&int_arg(args, 0)?)),
        (_, "partial_cmp") => O::SomeOrdering(
            as_f()
                .partial_cmp(&float_arg(args, 0)?)
                .unwrap_or(Ordering::Equal),
        ),
        _ => return Ok(None),
    }))
}

// -- chars -----------------------------------------------------------------

/// The result of a `char` method, in a form either engine can turn into its
/// own value type. Keeps the classification table in one place.
pub(super) enum CharOut {
    Bool(bool),
    Char(char),
    Str(String),
}

/// The `char` classification and conversion methods, shared by both engines so
/// a script sees the same set whichever one runs it.
pub(super) fn char_method(ch: char, name: &str) -> Option<CharOut> {
    let b = |v: bool| Some(CharOut::Bool(v));
    match name {
        "is_ascii_digit" => b(ch.is_ascii_digit()),
        "is_ascii_alphabetic" => b(ch.is_ascii_alphabetic()),
        "is_ascii_alphanumeric" => b(ch.is_ascii_alphanumeric()),
        "is_ascii_uppercase" => b(ch.is_ascii_uppercase()),
        "is_ascii_lowercase" => b(ch.is_ascii_lowercase()),
        "is_ascii_whitespace" => b(ch.is_ascii_whitespace()),
        "is_ascii_punctuation" => b(ch.is_ascii_punctuation()),
        "is_ascii_hexdigit" => b(ch.is_ascii_hexdigit()),
        "is_ascii" => b(ch.is_ascii()),
        "is_alphabetic" => b(ch.is_alphabetic()),
        "is_alphanumeric" => b(ch.is_alphanumeric()),
        "is_numeric" => b(ch.is_numeric()),
        "is_whitespace" => b(ch.is_whitespace()),
        "is_uppercase" => b(ch.is_uppercase()),
        "is_lowercase" => b(ch.is_lowercase()),
        "to_ascii_uppercase" => Some(CharOut::Char(ch.to_ascii_uppercase())),
        "to_ascii_lowercase" => Some(CharOut::Char(ch.to_ascii_lowercase())),
        // These yield an iterator in real Rust, but a script only ever renders
        // or collects it, so the string it would produce is handed back.
        "to_uppercase" => Some(CharOut::Str(ch.to_uppercase().to_string())),
        "to_lowercase" => Some(CharOut::Str(ch.to_lowercase().to_string())),
        _ => None,
    }
}

// -- strings ---------------------------------------------------------------

/// What a string method produced, materialized by each engine. `Keep` and
/// `OkKeep` hand the receiver back so both engines answer with a refcount
/// bump, never a copy.
pub(super) enum StrOut {
    Bool(bool),
    Int(i64),
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
    Parse(ParseNum),
}

/// The untyped `parse` guess: int first, then float, then bool.
pub(super) enum ParseNum {
    Int(i64),
    Float(f64),
    Bool(bool),
    Fail(String),
}

pub(super) fn str_core(s: &str, name: &str, args: &impl Args) -> Result<Option<StrOut>> {
    use StrOut as O;
    let a = |i: usize| args.text(i);
    Ok(Some(match name {
        "len" => O::Int(s.len() as i64),
        "is_empty" => O::Bool(s.is_empty()),
        "count" => O::Int(s.chars().count() as i64),
        "contains" => O::Bool(s.contains(&a(0))),
        "eq_ignore_ascii_case" => O::Bool(s.eq_ignore_ascii_case(&a(0))),
        "starts_with" => O::Bool(s.starts_with(&a(0))),
        "ends_with" => O::Bool(s.ends_with(&a(0))),
        "trim" => O::Owned(s.trim().to_string()),
        "trim_start" => O::Owned(s.trim_start().to_string()),
        "trim_end" => O::Owned(s.trim_end().to_string()),
        "to_uppercase" => O::Owned(s.to_uppercase()),
        "to_lowercase" => O::Owned(s.to_lowercase()),
        // The ascii variants leave non-ascii characters alone, they are not
        // aliases of the unicode ones.
        "to_ascii_uppercase" => O::Owned(s.to_ascii_uppercase()),
        "to_ascii_lowercase" => O::Owned(s.to_ascii_lowercase()),
        // A char-set pattern like `[':', '.']` replaces any of its members, matching real Rust. Without
        // this the array renders as text and matches nothing, silently leaving the string unchanged.
        "replace" => match args.pattern_chars(0) {
            Some(cs) => O::Owned(s.replace(cs.as_slice(), &a(1))),
            None => O::Owned(s.replace(&a(0), &a(1))),
        },
        "replacen" => match args.pattern_chars(0) {
            Some(cs) => O::Owned(s.replacen(cs.as_slice(), &a(1), int_arg(args, 2)? as usize)),
            None => O::Owned(s.replacen(&a(0), &a(1), int_arg(args, 2)? as usize)),
        },
        "repeat" => {
            let n = args
                .int(0)
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(0);
            O::Owned(s.repeat(n))
        }
        // String::as_str gives the string back. serde_json::Value::as_str
        // gives an Option, and a json string is a plain Str here, so unwrap
        // and expect on a string are identity to keep serde chains working.
        "to_owned" | "trim_string" | "as_str" | "as_string" | "unwrap" | "expect" => O::Keep,
        "unwrap_or" | "unwrap_or_else" | "unwrap_or_default" => O::Keep,
        // A String or a Cow that already owns its data, into_owned is self.
        "into_owned" | "into_string" => O::Keep,
        // `Option::context` returns a Result, so the pre-unwrapped string has
        // to come back wrapped or a following `?` would have nothing to unwrap.
        "context" | "with_context" => O::OkKeep,
        "is_some" => O::Bool(true),
        "is_none" => O::Bool(false),
        "as_bytes" | "into_bytes" => O::Ints(s.bytes().map(i64::from).collect()),
        // The utf-16 code units as an eager list of ints, mirroring `bytes`.
        "encode_utf16" => O::Ints(s.encode_utf16().map(i64::from).collect()),
        "strip_prefix" => O::OptOwned(s.strip_prefix(&a(0)).map(str::to_string)),
        "strip_suffix" => O::OptOwned(s.strip_suffix(&a(0)).map(str::to_string)),
        // Byte offsets, same as the real std, and slicing is byte based too,
        // so `&s[..s.find(x).unwrap()]` behaves right.
        "find" => O::OptInt(s.find(&a(0)).map(|i| i as i64)),
        "rfind" => O::OptInt(s.rfind(&a(0)).map(|i| i as i64)),
        "split_once" => O::OptPair(
            s.split_once(&a(0))
                .map(|(x, y)| (x.to_string(), y.to_string())),
        ),
        "rsplit_once" => O::OptPair(
            s.rsplit_once(&a(0))
                .map(|(x, y)| (x.to_string(), y.to_string())),
        ),
        // A char array like `['-', '_']` splits on any of its members, which
        // a plain string pattern would only match as the literal sequence.
        "split" => match args.pattern_chars(0) {
            Some(chars) => O::Strs(
                s.split(|c: char| chars.contains(&c))
                    .map(str::to_string)
                    .collect(),
            ),
            None => O::Strs(s.split(&a(0)).map(str::to_string).collect()),
        },
        "rsplit" => O::Strs(s.rsplit(&a(0)).map(str::to_string).collect()),
        "splitn" => {
            let n = int_arg(args, 0)? as usize;
            O::Strs(s.splitn(n, &a(1)).map(str::to_string).collect())
        }
        "rsplitn" => {
            let n = int_arg(args, 0)? as usize;
            O::Strs(s.rsplitn(n, &a(1)).map(str::to_string).collect())
        }
        "matches" => O::Strs(s.matches(&a(0)).map(str::to_string).collect()),
        "char_indices" => O::CharIdx(s.char_indices().map(|(i, c)| (i as i64, c)).collect()),
        "trim_matches" | "trim_start_matches" | "trim_end_matches" => {
            let pat = a(0);
            let out = match name {
                "trim_start_matches" => s.trim_start_matches(&pat),
                "trim_end_matches" => s.trim_end_matches(&pat),
                // trim_matches only takes chars in real Rust.
                _ => match args.pattern_chars(0) {
                    Some(chars) => s.trim_matches(|c: char| chars.contains(&c)),
                    None => s.trim_matches(pat.chars().next().unwrap_or(' ')),
                },
            };
            O::Owned(out.to_string())
        }
        "cmp" => O::Ordering(s.cmp(a(0).as_str())),
        "parse" => {
            let t = s.trim();
            if let Ok(i) = t.parse::<i64>() {
                O::Parse(ParseNum::Int(i))
            } else if let Ok(f) = t.parse::<f64>() {
                O::Parse(ParseNum::Float(f))
            } else if let Ok(b) = t.parse::<bool>() {
                O::Parse(ParseNum::Bool(b))
            } else {
                O::Parse(ParseNum::Fail(format!("cannot parse `{t}`")))
            }
        }
        _ => return Ok(None),
    }))
}

/// The `colored` crate as string methods, shared so tokio scripts color their
/// output the same way. Returns the styled text as a plain string carrying
/// ANSI codes, so chaining and printing both work. Honors the crate's own
/// NO_COLOR and terminal detection.
pub(super) fn color_core(s: &str, name: &str) -> Option<String> {
    use colored::Colorize;
    let out = match name {
        "red" => s.red(),
        "green" => s.green(),
        "yellow" => s.yellow(),
        "blue" => s.blue(),
        "magenta" | "purple" => s.magenta(),
        "cyan" => s.cyan(),
        "white" => s.white(),
        "black" => s.black(),
        "bright_red" => s.bright_red(),
        "bright_green" => s.bright_green(),
        "bright_yellow" => s.bright_yellow(),
        "bright_blue" => s.bright_blue(),
        "bright_cyan" => s.bright_cyan(),
        "on_red" => s.on_red(),
        "on_green" => s.on_green(),
        "on_blue" => s.on_blue(),
        "bold" => s.bold(),
        "dimmed" => s.dimmed(),
        "italic" => s.italic(),
        "underline" => s.underline(),
        "reversed" => s.reversed(),
        "clear" | "normal" => s.normal(),
        _ => return None,
    };
    Some(out.to_string())
}
