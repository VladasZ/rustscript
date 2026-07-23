use anyhow::{Result, bail};

use super::Interp;
use super::bytecode::Chunk;
use super::value::Value;

/// A script runtime abort. What a compiled binary would report as a panic,
/// carried as a typed error so `main` can print the panic header and exit
/// with the panic status 101, matching real Rust.
#[derive(Debug)]
pub struct ScriptPanic {
    /// File and line of the innermost script frame, for the panic header.
    pub file: String,
    pub line: u32,
    /// The message with the script backtrace lines appended.
    pub rendered: String,
}

impl std::fmt::Display for ScriptPanic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.rendered)
    }
}

impl std::error::Error for ScriptPanic {}

/// `main` returned a `Result::Err` value. A compiled binary prints it as
/// `Error: ...` and exits 1, so this marks that outcome apart from a panic.
#[derive(Debug)]
pub struct ErrReturn(pub String);

impl std::fmt::Display for ErrReturn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Error: {}", self.0)
    }
}

impl std::error::Error for ErrReturn {}

/// Wrap a runtime error as a `ScriptPanic` carrying the script backtrace.
/// Frames arrive innermost first as (function, file, line), line 0 meaning
/// unknown. Deep chains cap at a fixed count so runaway recursion stays
/// readable.
pub(super) fn trace_error(
    e: anyhow::Error,
    frames: impl Iterator<Item = (String, String, u32)>,
) -> anyhow::Error {
    const SHOWN: usize = 15;
    let mut msg = format!("{e:#}");
    // A closure called from inside a bridge runs its own exec and wraps
    // first; the panic origin must stay that innermost site, not this
    // outer exec's current frame.
    let mut origin: Option<(String, u32)> = e
        .downcast_ref::<ScriptPanic>()
        .map(|p| (p.file.clone(), p.line));
    let mut hidden = 0usize;
    for (i, (func, file, line)) in frames.enumerate() {
        if origin.is_none() {
            origin = Some((file.clone(), line));
        }
        if i >= SHOWN {
            hidden += 1;
            continue;
        }
        if file.is_empty() {
            msg.push_str(&format!("\n  at {func}"));
        } else if line == 0 {
            msg.push_str(&format!("\n  at {func} ({file})"));
        } else {
            msg.push_str(&format!("\n  at {func} ({file}:{line})"));
        }
    }
    if hidden > 0 {
        msg.push_str(&format!("\n  ... {hidden} more frames"));
    }
    let (file, line) = origin.unwrap_or_default();
    anyhow::Error::new(ScriptPanic {
        file,
        line,
        rendered: msg,
    })
}

impl Interp {
    pub(super) fn internal_path(
        &self,
        segments: &[String],
        registers: &[Value],
        base: usize,
        count: usize,
    ) -> Result<Option<Value>> {
        let head = segments.first().map(String::as_str).unwrap_or("");
        match head {
            "::unreachable_match" => bail!("no match arm matched the value"),
            "::assert_failed" => bail!("assertion failed"),
            "::ensure_fail" => {
                let message = if count > 0 {
                    registers[base].display()
                } else {
                    "condition failed".to_string()
                };
                Ok(Some(Value::err(Value::str(message))))
            }
            _ => Ok(None),
        }
    }

    pub(super) fn render_fmt(
        &self,
        chunk: &Chunk,
        specification: u16,
        registers: &[Value],
    ) -> Result<String> {
        let format = &chunk.fmts[specification as usize];
        let positional = format
            .positional
            .iter()
            .map(|register| registers[*register as usize].clone())
            .collect::<Vec<_>>();
        let named = format
            .named
            .iter()
            .map(|(name, register)| (name.clone(), registers[*register as usize].clone()))
            .collect::<Vec<_>>();
        super::format::render_values(&format.template, &positional, &named)
    }
}

pub(super) fn take_range(stack: &mut [Value], start: usize, count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| take(&mut stack[start + index]))
        .collect()
}

#[inline(always)]
pub(super) fn set_reg(slot: &mut Value, value: Value) {
    if matches!(
        slot,
        Value::Unit
            | Value::Bool(_)
            | Value::Int(_)
            | Value::Float(_)
            | Value::Char(_)
            | Value::Range { .. }
    ) {
        std::mem::forget(replace(slot, value));
    } else {
        *slot = value;
    }
}

pub(super) fn int_of(value: &Value, description: &str) -> Result<i64> {
    match value {
        Value::Int(value) => Ok(*value),
        _ => bail!("{description} must be an integer"),
    }
}
use std::mem::{replace, take};
