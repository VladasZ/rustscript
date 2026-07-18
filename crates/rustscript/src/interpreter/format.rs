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

/// Apply a subset of the format spec: debug flag, precision, width, and fill.
fn format_value(value: &Value, spec: &str) -> Result<String> {
    let number = match value {
        Value::Float(f) => Some(*f),
        Value::Int(i) => Some(*i as f64),
        _ => None,
    };
    Ok(apply_spec(spec, &value.display(), &value.debug(), number))
}

/// Apply the precision, width, alignment and fill parts of a spec to a value
/// that has already been rendered. Shared by both engines so a script formats
/// the same whichever one runs it.
pub(super) fn apply_spec(spec: &str, display: &str, debug: &str, number: Option<f64>) -> String {
    let is_debug = spec.contains('?');
    let spec = spec.replace(['#', '?'], "");

    let mut base = if is_debug {
        debug.to_string()
    } else {
        // Precision applies to numbers as decimal places.
        match (spec.split_once('.'), number) {
            (Some((_, prec)), Some(f)) => {
                let prec: usize = prec
                    .trim_end_matches(|c: char| !c.is_ascii_digit())
                    .parse()
                    .unwrap_or(0);
                format!("{f:.prec$}")
            }
            _ => display.to_string(),
        }
    };
    let value_is_numeric = number.is_some();

    // Width and alignment, `{:>8}`, `{:<8}`, `{:^8}`, `{:08}`. Numbers default
    // to right aligned and everything else to left, same as real Rust.
    let numeric = value_is_numeric;
    let width_part = spec.split('.').next().unwrap_or("");
    let (align, rest) = split_align(width_part, numeric);
    let zero = rest.starts_with('0');
    let rest = rest.trim_start_matches('0');
    if let Ok(width) = rest.parse::<usize>()
        && base.len() < width
    {
        let pad = width - base.len();
        let fill = if zero && align == '>' { '0' } else { ' ' };
        match align {
            '<' => base = format!("{base}{}", fill_str(fill, pad)),
            '^' => {
                let left = pad / 2;
                base = format!(
                    "{}{base}{}",
                    fill_str(fill, left),
                    fill_str(fill, pad - left)
                );
            }
            _ => base = format!("{}{base}", fill_str(fill, pad)),
        }
    }
    base
}

fn split_align(spec: &str, numeric: bool) -> (char, &str) {
    let mut chars = spec.chars();
    // An explicit fill character sits before the alignment, as in `{:*>8}`.
    let rest = chars.as_str();
    if let Some(a) = chars.clone().nth(1)
        && matches!(a, '<' | '>' | '^')
        && let Some(fill) = chars.next()
        && fill != '<'
        && fill != '>'
        && fill != '^'
    {
        chars.next();
        return (a, chars.as_str());
    }
    let mut chars = rest.chars();
    match chars.next() {
        Some(a @ ('<' | '>' | '^')) => (a, chars.as_str()),
        _ => (if numeric { '>' } else { '<' }, rest),
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
        other => Err(anyhow!(
            "format width must be an integer, got {}",
            other.type_name()
        )),
    }
}

fn fill_str(c: char, n: usize) -> String {
    std::iter::repeat_n(c, n).collect()
}
