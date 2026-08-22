//! Format spec parsing and rendering for the VM's `Fmt` op.

use anyhow::Result;
use num_traits::AsPrimitive;

/// Radix forms need the exact integer, and integers ignore precision where
/// floats round by it.
#[derive(Clone, Copy)]
pub(super) enum SpecNumber {
    Int(i64),
    /// Radix forms print the image at that width, `{:x}` of `-1i8` is `ff`.
    Sized {
        value: i128,
        bits: u32,
    },
    /// Raw storage bits. Only the exponent forms need the sign.
    Big {
        bits: i128,
        signed: bool,
    },
    Float(f64),
    F32(f32),
}

impl SpecNumber {
    /// Masked to the value's own width.
    fn radix_bits(value: i128, bits: u32) -> u64 {
        AsPrimitive::<u64>::as_(value) & (u64::MAX >> (64 - bits))
    }
}

/// `[[fill]align][+][#][0][width][.precision][type]`.
struct ParsedSpec {
    fill: char,
    align: Option<char>,
    plus: bool,
    alternate: bool,
    zero: bool,
    width: Option<usize>,
    precision: Option<usize>,
    repr: Repr,
    ty: Option<char>,
}

#[derive(PartialEq)]
enum Repr {
    Display,
    Debug,
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
        repr: Repr::Display,
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
    // `-` is accepted and does nothing.
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
            '?' => parsed.repr = Repr::Debug,
            'x' | 'X' | 'o' | 'b' | 'e' | 'E' => parsed.ty = Some(c),
            _ => {}
        }
    }
    parsed
}

pub(super) fn apply_spec(
    spec: &str,
    display: &str,
    debug: &str,
    number: Option<SpecNumber>,
    pads_debug: bool,
) -> String {
    let parsed = parse_spec(spec);
    let mut base = render_base(&parsed, display, debug, number);
    // The `Debug` of str, char and containers never pads, only numbers and
    // bool route `Debug` to a padding `Display`.
    if parsed.repr == Repr::Debug && !pads_debug {
        return base;
    }

    // `{:+}` of NaN is still `NaN`, infinities take the sign.
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
        && matches!(
            number,
            Some(SpecNumber::Int(_) | SpecNumber::Sized { .. } | SpecNumber::Big { .. })
        )
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
    // The zero flag pads after the sign and prefix, `{:#010x}` gives
    // `0x000000ff`, and wins over an explicit fill.
    if parsed.zero && number.is_some() {
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

/// Type conversion and precision, no width yet.
fn render_base(
    parsed: &ParsedSpec,
    display: &str,
    debug: &str,
    number: Option<SpecNumber>,
) -> String {
    if parsed.repr == Repr::Debug {
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
        (Some('x'), Some(SpecNumber::Big { bits, .. })) => format!("{:x}", bits.cast_unsigned()),
        (Some('X'), Some(SpecNumber::Big { bits, .. })) => format!("{:X}", bits.cast_unsigned()),
        (Some('o'), Some(SpecNumber::Big { bits, .. })) => format!("{:o}", bits.cast_unsigned()),
        (Some('b'), Some(SpecNumber::Big { bits, .. })) => format!("{:b}", bits.cast_unsigned()),
        (Some(exp @ ('e' | 'E')), Some(SpecNumber::Big { bits, signed })) => {
            big_exponent(bits, signed, exp == 'E', parsed.precision)
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
        (_, Some(SpecNumber::Int(_) | SpecNumber::Sized { .. } | SpecNumber::Big { .. })) => {
            display.to_string()
        }
        (_, None) => match parsed.precision {
            Some(precision) => display.chars().take(precision).collect(),
            None => display.to_string(),
        },
    }
}

/// `{:e}` of a 128 bit integer, in the value's real sign.
fn big_exponent(bits: i128, signed: bool, upper: bool, precision: Option<usize>) -> String {
    match (signed, upper, precision) {
        (true, false, Some(p)) => format!("{bits:.p$e}"),
        (true, false, None) => format!("{bits:e}"),
        (true, true, Some(p)) => format!("{bits:.p$E}"),
        (true, true, None) => format!("{bits:E}"),
        (false, false, Some(p)) => format!("{:.p$e}", bits.cast_unsigned()),
        (false, false, None) => format!("{:e}", bits.cast_unsigned()),
        (false, true, Some(p)) => format!("{:.p$E}", bits.cast_unsigned()),
        (false, true, None) => format!("{:E}", bits.cast_unsigned()),
    }
}

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
            // `0w$` is the zero flag plus a width reference, an argument index
            // never has a leading zero.
            if token.len() > 1 && token.starts_with('0') {
                out.push('0');
                token.remove(0);
            }
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

fn fill_str(c: char, n: usize) -> String {
    std::iter::repeat_n(c, n).collect()
}
