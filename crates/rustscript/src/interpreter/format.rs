use anyhow::{Result, anyhow};

use super::value::Value;

/// Render a format template. Positional and named arguments, including inline
/// `{name}` holes, are already evaluated by the compiler.
pub(super) fn render_values(
    template: &str,
    positional: &[Value],
    named: &[(String, Value)],
) -> Result<String> {
    let mut out = String::new();
    let mut chars = template.chars().peekable();
    let mut next_positional = 0;

    while let Some(c) = chars.next() {
        match c {
            '{' => {
                if chars.peek() == Some(&'{') {
                    chars.next();
                    out.push('{');
                    continue;
                }
                let mut inner = String::new();
                for ic in chars.by_ref() {
                    if ic == '}' {
                        break;
                    }
                    inner.push(ic);
                }
                let (arg_ref, spec) = match inner.split_once(':') {
                    Some((a, s)) => (a.trim(), s),
                    None => (inner.trim(), ""),
                };
                let value = resolve_arg(arg_ref, &mut next_positional, positional, named)?;
                let spec = expand_arg_widths(spec, positional, named)?;
                out.push_str(&format_value(&value, &spec)?);
            }
            '}' => {
                if chars.peek() == Some(&'}') {
                    chars.next();
                }
                out.push('}');
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

fn resolve_arg(
    arg_ref: &str,
    next_positional: &mut usize,
    positional: &[Value],
    named: &[(String, Value)],
) -> Result<Value> {
    if arg_ref.is_empty() {
        let idx = *next_positional;
        *next_positional += 1;
        return positional
            .get(idx)
            .cloned()
            .ok_or_else(|| anyhow!("not enough arguments for format string"));
    }
    if let Ok(idx) = arg_ref.parse::<usize>() {
        return positional
            .get(idx)
            .cloned()
            .ok_or_else(|| anyhow!("format argument {idx} out of range"));
    }
    named
        .iter()
        .find(|(n, _)| n == arg_ref)
        .map(|(_, v)| v.clone())
        .ok_or_else(|| anyhow!("`{arg_ref}` not found for format string"))
}

/// Apply the format spec to an already-evaluated value.
fn format_value(value: &Value, spec: &str) -> Result<String> {
    let number = match value {
        Value::Float(f) => Some(SpecNumber::Float(*f)),
        Value::F32(f) => Some(SpecNumber::F32(*f)),
        Value::Int(i) => Some(SpecNumber::Int(*i)),
        Value::IntW(v, w) => Some(SpecNumber::Sized {
            value: w.decode(*v),
            bits: w.bits(),
        }),
        _ => None,
    };
    Ok(apply_spec(spec, &value.display(), &value.debug(), number))
}

/// The numeric identity of a formatted value. Radix forms need the exact
/// integer, an f64 would lose the low bits past 2^53, and integers must
/// ignore precision where floats round by it.
#[derive(Clone, Copy)]
pub(super) enum SpecNumber {
    Int(i64),
    /// A width-tagged integer. Radix forms print the two's complement image
    /// at that width, `{:x}` of `-1i8` is `ff`, not 16 f's.
    Sized {
        value: i128,
        bits: u32,
    },
    Float(f64),
    F32(f32),
}

impl SpecNumber {
    /// The bits radix forms print, masked to the value's own width.
    fn radix_bits(value: i128, bits: u32) -> u64 {
        (value as u64) & (u64::MAX >> (64 - bits))
    }
}

/// One parsed `{:...}` spec: `[[fill]align][+][#][0][width][.precision][type]`.
struct ParsedSpec {
    fill: char,
    align: Option<char>,
    plus: bool,
    alternate: bool,
    zero: bool,
    width: Option<usize>,
    precision: Option<usize>,
    debug: bool,
    ty: Option<char>,
}

fn parse_spec(spec: &str) -> ParsedSpec {
    let chars: Vec<char> = spec.chars().collect();
    let mut parsed = ParsedSpec {
        fill: ' ',
        align: None,
        plus: false,
        alternate: false,
        zero: false,
        width: None,
        precision: None,
        debug: false,
        ty: None,
    };
    let mut index = 0;
    let is_align = |c: char| matches!(c, '<' | '>' | '^');
    if chars.len() > index + 1 && is_align(chars[index + 1]) {
        parsed.fill = chars[index];
        parsed.align = Some(chars[index + 1]);
        index += 2;
    } else if chars.get(index).copied().is_some_and(is_align) {
        parsed.align = Some(chars[index]);
        index += 1;
    }
    if chars.get(index) == Some(&'+') {
        parsed.plus = true;
        index += 1;
    }
    // `-` is accepted by real Rust and does nothing.
    if chars.get(index) == Some(&'-') {
        index += 1;
    }
    if chars.get(index) == Some(&'#') {
        parsed.alternate = true;
        index += 1;
    }
    if chars.get(index) == Some(&'0') {
        parsed.zero = true;
        index += 1;
    }
    let mut width = String::new();
    while chars.get(index).is_some_and(char::is_ascii_digit) {
        width.push(chars[index]);
        index += 1;
    }
    if !width.is_empty() {
        parsed.width = width.parse().ok();
    }
    if chars.get(index) == Some(&'.') {
        index += 1;
        let mut precision = String::new();
        while chars.get(index).is_some_and(char::is_ascii_digit) {
            precision.push(chars[index]);
            index += 1;
        }
        parsed.precision = Some(precision.parse().unwrap_or(0));
    }
    for &c in &chars[index.min(chars.len())..] {
        match c {
            '?' => parsed.debug = true,
            'x' | 'X' | 'o' | 'b' | 'e' | 'E' => parsed.ty = Some(c),
            _ => {}
        }
    }
    parsed
}

/// Apply a spec to a value that has already been rendered. Shared by both
/// engines so a script formats the same whichever one runs it. Covers debug,
/// precision, width, fill, alignment, sign, sign-aware zero padding, radix
/// and exponent types, and their `#` alternate forms.
pub(super) fn apply_spec(
    spec: &str,
    display: &str,
    debug: &str,
    number: Option<SpecNumber>,
) -> String {
    let parsed = parse_spec(spec);
    let mut base = render_base(&parsed, display, debug, number);

    // NaN ignores the sign flag entirely, `{:+}` of NaN is still `NaN`,
    // while infinities do take it.
    let is_nan = match number {
        Some(SpecNumber::Float(f)) => f.is_nan(),
        Some(SpecNumber::F32(f)) => f.is_nan(),
        _ => false,
    };
    if parsed.plus && number.is_some() && !is_nan && !base.starts_with('-') {
        base.insert(0, '+');
    }
    if parsed.alternate
        && let Some(ty @ ('x' | 'X' | 'o' | 'b')) = parsed.ty
        && matches!(number, Some(SpecNumber::Int(_) | SpecNumber::Sized { .. }))
    {
        let prefix = match ty {
            'x' | 'X' => "0x",
            'o' => "0o",
            _ => "0b",
        };
        let after_sign = usize::from(base.starts_with('+') || base.starts_with('-'));
        base.insert_str(after_sign, prefix);
    }

    let Some(target) = parsed.width else {
        return base;
    };
    let current = base.chars().count();
    if current >= target {
        return base;
    }
    let pad = target - current;
    // The zero flag pads after the sign and radix prefix, `{:+06}` gives
    // `+00013` and `{:#010x}` gives `0x000000ff`, unlike an explicit fill.
    if parsed.zero && parsed.align.is_none() && number.is_some() {
        let mut cut = usize::from(base.starts_with('+') || base.starts_with('-'));
        if base[cut..].starts_with("0x")
            || base[cut..].starts_with("0o")
            || base[cut..].starts_with("0b")
        {
            cut += 2;
        }
        base.insert_str(cut, &fill_str('0', pad));
        return base;
    }
    let align = parsed
        .align
        .unwrap_or(if number.is_some() { '>' } else { '<' });
    let fill = parsed.fill;
    match align {
        '<' => format!("{base}{}", fill_str(fill, pad)),
        '^' => {
            let left = pad / 2;
            format!(
                "{}{base}{}",
                fill_str(fill, left),
                fill_str(fill, pad - left)
            )
        }
        _ => format!("{}{base}", fill_str(fill, pad)),
    }
}

/// The unpadded rendering: type conversion and precision, no width yet.
fn render_base(
    parsed: &ParsedSpec,
    display: &str,
    debug: &str,
    number: Option<SpecNumber>,
) -> String {
    if parsed.debug {
        return debug.to_string();
    }
    match (parsed.ty, number) {
        (Some('x'), Some(SpecNumber::Int(i))) => format!("{i:x}"),
        (Some('X'), Some(SpecNumber::Int(i))) => format!("{i:X}"),
        (Some('o'), Some(SpecNumber::Int(i))) => format!("{i:o}"),
        (Some('b'), Some(SpecNumber::Int(i))) => format!("{i:b}"),
        (Some('x'), Some(SpecNumber::Sized { value, bits })) => {
            format!("{:x}", SpecNumber::radix_bits(value, bits))
        }
        (Some('X'), Some(SpecNumber::Sized { value, bits })) => {
            format!("{:X}", SpecNumber::radix_bits(value, bits))
        }
        (Some('o'), Some(SpecNumber::Sized { value, bits })) => {
            format!("{:o}", SpecNumber::radix_bits(value, bits))
        }
        (Some('b'), Some(SpecNumber::Sized { value, bits })) => {
            format!("{:b}", SpecNumber::radix_bits(value, bits))
        }
        (Some('e'), Some(SpecNumber::Int(i))) => match parsed.precision {
            Some(precision) => format!("{i:.precision$e}"),
            None => format!("{i:e}"),
        },
        (Some('e'), Some(SpecNumber::Sized { value, .. })) => match parsed.precision {
            Some(precision) => format!("{value:.precision$e}"),
            None => format!("{value:e}"),
        },
        (Some('e'), Some(SpecNumber::Float(f))) => match parsed.precision {
            Some(precision) => format!("{f:.precision$e}"),
            None => format!("{f:e}"),
        },
        (Some('e'), Some(SpecNumber::F32(f))) => match parsed.precision {
            Some(precision) => format!("{f:.precision$e}"),
            None => format!("{f:e}"),
        },
        (Some('E'), Some(SpecNumber::Int(i))) => match parsed.precision {
            Some(precision) => format!("{i:.precision$E}"),
            None => format!("{i:E}"),
        },
        (Some('E'), Some(SpecNumber::Sized { value, .. })) => match parsed.precision {
            Some(precision) => format!("{value:.precision$E}"),
            None => format!("{value:E}"),
        },
        (Some('E'), Some(SpecNumber::Float(f))) => match parsed.precision {
            Some(precision) => format!("{f:.precision$E}"),
            None => format!("{f:E}"),
        },
        (Some('E'), Some(SpecNumber::F32(f))) => match parsed.precision {
            Some(precision) => format!("{f:.precision$E}"),
            None => format!("{f:E}"),
        },
        (_, Some(SpecNumber::Float(f))) => match parsed.precision {
            Some(precision) => format!("{f:.precision$}"),
            None => display.to_string(),
        },
        (_, Some(SpecNumber::F32(f))) => match parsed.precision {
            Some(precision) => format!("{f:.precision$}"),
            None => display.to_string(),
        },
        // Integer Display ignores precision.
        (_, Some(SpecNumber::Int(_) | SpecNumber::Sized { .. })) => display.to_string(),
        // String precision truncates to that many characters.
        (_, None) => match parsed.precision {
            Some(precision) => display.chars().take(precision).collect(),
            None => display.to_string(),
        },
    }
}

/// Replace `name$` and `0$` width or precision references with their value, so
/// `{:w$}` pads by whatever `w` holds at render time.
pub(super) fn expand_widths_with(
    spec: &str,
    lookup: &mut dyn FnMut(&str) -> Result<i64>,
) -> Result<String> {
    if !spec.contains('$') {
        return Ok(spec.to_string());
    }
    let mut out = String::new();
    let mut token = String::new();
    for c in spec.chars() {
        if c.is_alphanumeric() || c == '_' {
            token.push(c);
            continue;
        }
        if c == '$' {
            out.push_str(&lookup(&token)?.to_string());
            token.clear();
            continue;
        }
        out.push_str(&token);
        token.clear();
        out.push(c);
    }
    out.push_str(&token);
    Ok(out)
}

fn expand_arg_widths(
    spec: &str,
    positional: &[Value],
    named: &[(String, Value)],
) -> Result<String> {
    if !spec.contains('$') {
        return Ok(spec.to_string());
    }
    let mut out = String::new();
    let mut token = String::new();
    for c in spec.chars() {
        if c.is_alphanumeric() || c == '_' {
            token.push(c);
            continue;
        }
        if c == '$' {
            out.push_str(&width_arg(&token, positional, named)?.to_string());
            token.clear();
            continue;
        }
        out.push_str(&token);
        token.clear();
        out.push(c);
    }
    out.push_str(&token);
    Ok(out)
}

fn width_arg(token: &str, positional: &[Value], named: &[(String, Value)]) -> Result<i64> {
    let value = match token.parse::<usize>() {
        Ok(idx) => positional
            .get(idx)
            .cloned()
            .ok_or_else(|| anyhow!("format width argument {idx} out of range"))?,
        Err(_) => named
            .iter()
            .find(|(n, _)| n == token)
            .map(|(_, v)| v.clone())
            .ok_or_else(|| anyhow!("`{token}` not found for a format width"))?,
    };
    match value {
        Value::Int(i) => Ok(i),
        Value::IntW(..) => value
            .untag_int()
            .ok_or_else(|| anyhow!("format width out of range")),
        other => Err(anyhow!(
            "format width must be an integer, got {}",
            other.type_name()
        )),
    }
}

fn fill_str(c: char, n: usize) -> String {
    std::iter::repeat_n(c, n).collect()
}
