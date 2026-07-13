use anyhow::{Result, bail};

use super::Interp;
use super::bytecode::Chunk;
use super::value::Value;

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
