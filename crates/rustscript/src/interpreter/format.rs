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
                out.push_str(&format_value(&value, spec)?);
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
    let debug = spec.contains('?');
    let spec = spec.replace(['#', '?'], "");

    let mut base = if debug {
        value.debug()
    } else {
        // Precision applies to floats as decimal places.
        if let Some((_, prec)) = spec.split_once('.') {
            let prec: usize = prec
                .trim_end_matches(|c: char| !c.is_ascii_digit())
                .parse()
                .unwrap_or(0);
            match value {
                Value::Float(f) => format!("{f:.prec$}"),
                Value::Int(i) => format!("{:.prec$}", *i as f64),
                other => other.display(),
            }
        } else {
            value.display()
        }
    };

    // Width and alignment, `{:>8}`, `{:<8}`, `{:^8}`, `{:08}`.
    let width_part = spec.split('.').next().unwrap_or("");
    let (align, rest) = split_align(width_part);
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
    Ok(base)
}

fn split_align(spec: &str) -> (char, &str) {
    let mut chars = spec.chars();
    match chars.next() {
        Some(a @ ('<' | '>' | '^')) => (a, chars.as_str()),
        _ => ('>', spec),
    }
}

fn fill_str(c: char, n: usize) -> String {
    std::iter::repeat_n(c, n).collect()
}
