//! Format template rendering for the `Fmt` op.

use std::sync::Arc;

use anyhow::{Result, bail};

use crate::interpreter::value::Value;
use crate::interpreter::vm::Vm;

// template rendering

pub(super) fn render_template(
    vm: &Arc<Vm>,
    template: &str,
    positional: &[Value],
    named: &[(&str, Value)],
) -> Result<String> {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    let mut next_pos = 0usize;
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut spec = String::new();
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                    spec.push(c);
                }
                out.push_str(&render_placeholder(
                    vm,
                    &spec,
                    &mut next_pos,
                    positional,
                    named,
                )?);
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

pub(super) fn render_placeholder(
    vm: &Arc<Vm>,
    spec: &str,
    next_pos: &mut usize,
    positional: &[Value],
    named: &[(&str, Value)],
) -> Result<String> {
    let (name, fmt) = spec.split_once(':').unwrap_or((spec, ""));
    // `{:.*}` takes its precision from the next positional argument
    let fmt = if fmt.contains(".*") {
        let precision = match resolve_arg("", next_pos, positional, named)? {
            Value::Int(i) => i,
            ref other @ Value::IntW(..) => other
                .untag_int()
                .ok_or_else(|| anyhow::anyhow!("format precision out of range"))?,
            other => {
                bail!(
                    "format precision must be an integer, got {}",
                    other.type_name()
                )
            }
        };
        fmt.replace(".*", &format!(".{precision}"))
    } else {
        fmt.to_string()
    };
    let fmt = fmt.as_str();
    let value = resolve_arg(name, next_pos, positional, named)?;
    // a `{:w$}` width names another argument
    let mut lookup = |token: &str| -> Result<i64> {
        let mut pos = 0;
        match resolve_arg(token, &mut pos, positional, named)? {
            Value::Int(i) => Ok(i),
            ref other @ Value::IntW(..) => other
                .untag_int()
                .ok_or_else(|| anyhow::anyhow!("format width out of range")),
            other => {
                bail!("format width must be an integer, got {}", other.type_name())
            }
        }
    };
    let fmt = crate::interpreter::format::expand_widths_with(fmt, &mut lookup)?;
    let number = match &value {
        Value::Float(f) => Some(crate::interpreter::format::SpecNumber::Float(*f)),
        Value::F32(f) => Some(crate::interpreter::format::SpecNumber::F32(*f)),
        Value::Int(i) => Some(crate::interpreter::format::SpecNumber::Int(*i)),
        Value::IntW(v, w) => Some(crate::interpreter::format::SpecNumber::Sized {
            value: w.decode(*v),
            bits: w.bits(),
        }),
        Value::Big(v, w) => Some(crate::interpreter::format::SpecNumber::Big {
            bits: *v,
            signed: w.is_signed(),
        }),
        _ => None,
    };
    // only the form the spec asks for runs, an impl may have side effects
    let wants_debug = fmt.contains('?');
    // `write!` ignores the caller's width, `f.pad` honors it
    let mut user_padded = None;
    let display_text = if wants_debug {
        String::new()
    } else {
        match vm.user_fmt(&value, false)? {
            Some((text, padded)) => {
                user_padded = Some(padded);
                text
            }
            None => value.display(),
        }
    };
    let mut user_debug = false;
    let debug_text = if !wants_debug {
        String::new()
    } else if let Some((text, padded)) = vm.user_fmt(&value, true)? {
        user_debug = true;
        user_padded = Some(padded);
        text
    } else {
        // the flags reach every leaf
        let leaf: String = fmt.chars().filter(|c| !matches!(c, '#' | '?')).collect();
        crate::interpreter::debug_fmt::render(
            &value,
            &crate::interpreter::debug_fmt::DebugOpts {
                pretty: fmt.contains('#'),
                leaf: &leaf,
            },
        )
    };
    // the debug renderer applied the spec at every leaf already
    if wants_debug && !user_debug {
        return Ok(debug_text);
    }
    if user_padded == Some(false) {
        return Ok(if wants_debug {
            debug_text
        } else {
            display_text
        });
    }
    Ok(crate::interpreter::format::apply_spec(
        &fmt,
        &display_text,
        &debug_text,
        number,
        user_padded.is_some(),
    ))
}

pub(super) fn resolve_arg(
    name: &str,
    next_pos: &mut usize,
    positional: &[Value],
    named: &[(&str, Value)],
) -> Result<Value> {
    if name.is_empty() {
        let i = *next_pos;
        *next_pos += 1;
        return positional
            .get(i)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("format argument {i} is missing"));
    }
    if let Ok(i) = name.parse::<usize>() {
        return positional
            .get(i)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("format argument {i} is missing"));
    }
    named
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v.clone())
        .ok_or_else(|| anyhow::anyhow!("format name `{name}` is missing"))
}
