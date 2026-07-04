use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};
use syn::punctuated::Punctuated;
use syn::{Expr, Lit};

use super::eval::flow_value;
use super::value::Value;
use super::{Frame, Interp};

/// Render a `println!`/`format!` style macro to a string.
impl Interp {
    pub(super) fn expand_format(&self, mac: &syn::Macro, frame: &mut Frame) -> Result<String> {
        let args = mac.parse_body_with(Punctuated::<Expr, syn::Token![,]>::parse_terminated)?;
        let mut iter = args.iter();

        let template = match iter.next() {
            Some(Expr::Lit(lit)) => match &lit.lit {
                Lit::Str(s) => s.value(),
                other => bail!("format template must be a string literal, got {:?}", other),
            },
            Some(_) => bail!("format template must be a string literal"),
            None => return Ok(String::new()),
        };

        let mut positional = Vec::new();
        let mut named: HashMap<String, Value> = HashMap::new();
        for arg in iter {
            if let Expr::Assign(a) = arg
                && let Expr::Path(p) = &*a.left
                && let Some(name) = p.path.get_ident()
            {
                let v = flow_value(self.eval_expr(&a.right, frame)?)?;
                named.insert(name.to_string(), v);
                continue;
            }
            positional.push(flow_value(self.eval_expr(arg, frame)?)?);
        }

        render(&template, &positional, &named, frame)
    }
}

fn render(
    template: &str,
    positional: &[Value],
    named: &HashMap<String, Value>,
    frame: &Frame,
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
                let value = resolve_arg(arg_ref, &mut next_positional, positional, named, frame)?;
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
    named: &HashMap<String, Value>,
    frame: &Frame,
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
    if let Some(v) = named.get(arg_ref) {
        return Ok(v.clone());
    }
    // Inline captured identifier, `{name}`.
    frame
        .get(arg_ref)
        .ok_or_else(|| anyhow!("`{arg_ref}` not found for format string"))
}

/// Apply a subset of the format spec: debug flag, precision, width, and fill.
fn format_value(value: &Value, spec: &str) -> Result<String> {
    let debug = spec.contains('?');
    let spec = spec.replace('#', "").replace('?', "");

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
