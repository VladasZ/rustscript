//! The `String` methods that edit the receiver. The vm runs them on the receiver register or
//! through a reference or a shared cell, then stores the edited text back. The panic texts are
//! the std ones.

use std::sync::Arc;

use anyhow::{Result, bail};

use super::bridge::arg;
use super::bytecode::BuiltinId;
use super::iterator::as_closure;
use super::ops::{char_boundary_error, range_bounds};
use super::rs_str::RsStr;
use super::value::Value;
use super::vecmap::int_arg;
use super::vm::Vm;

pub(super) fn is_str_edit(id: BuiltinId) -> bool {
    matches!(
        id,
        BuiltinId::ReplaceRange
            | BuiltinId::InsertStr
            | BuiltinId::Insert
            | BuiltinId::Truncate
            | BuiltinId::Remove
            | BuiltinId::Pop
            | BuiltinId::Retain
    )
}

impl Vm {
    /// None when `id` is not an edit. The edited text and what the method returns.
    pub(super) fn str_edit(
        self: &Arc<Self>,
        text: &str,
        id: BuiltinId,
        args: &[Value],
    ) -> Result<Option<(RsStr, Value)>> {
        if !is_str_edit(id) {
            return Ok(None);
        }
        let mut s = text.to_string();
        let out = match id {
            BuiltinId::ReplaceRange => {
                replace_range(&mut s, args)?;
                Value::Unit
            }
            BuiltinId::InsertStr => {
                let idx = insert_index(&s, args)?;
                s.insert_str(idx, &arg(args, 1)?.display());
                Value::Unit
            }
            BuiltinId::Insert => {
                let idx = insert_index(&s, args)?;
                let Value::Char(c) = arg(args, 1)? else {
                    bail!("insert takes a char");
                };
                s.insert(idx, c);
                Value::Unit
            }
            BuiltinId::Truncate => {
                let new_len = byte_index(args)?;
                if new_len <= s.len() {
                    if !s.is_char_boundary(new_len) {
                        bail!("assertion failed: self.is_char_boundary(new_len)");
                    }
                    s.truncate(new_len);
                }
                Value::Unit
            }
            BuiltinId::Remove => Value::Char(remove(&mut s, args)?),
            BuiltinId::Pop => match s.pop() {
                Some(c) => Value::some(Value::Char(c)),
                None => Value::none(),
            },
            _ => {
                let f = as_closure(args.first())?;
                let mut kept = String::with_capacity(s.len());
                for c in s.chars() {
                    if self.call_closure_data(&f, &[Value::Char(c)])?.is_truthy() {
                        kept.push(c);
                    }
                }
                s = kept;
                Value::Unit
            }
        };
        Ok(Some((s.into(), out)))
    }

    /// An edit through a reference or a shared cell stores the text back into it. None when the
    /// receiver is neither or does not hold a string.
    pub(super) fn str_edit_in_place(
        self: &Arc<Self>,
        recv: &Value,
        id: BuiltinId,
        args: &[Value],
    ) -> Result<Option<Value>> {
        if !is_str_edit(id) {
            return Ok(None);
        }
        match recv {
            Value::Ref(reference) => {
                let Some(Value::Str(text)) = reference.get() else {
                    return Ok(None);
                };
                let Some((edited, out)) = self.str_edit(&text, id, args)? else {
                    return Ok(None);
                };
                reference.set(Value::Str(edited));
                Ok(Some(out))
            }
            Value::Cell(_, slot) => {
                // the lock is released before the call, a `retain` closure may read the cell
                let Value::Str(text) = slot.lock().clone() else {
                    return Ok(None);
                };
                let Some((edited, out)) = self.str_edit(&text, id, args)? else {
                    return Ok(None);
                };
                *slot.lock() = Value::Str(edited);
                Ok(Some(out))
            }
            _ => Ok(None),
        }
    }
}

fn byte_index(args: &[Value]) -> Result<usize> {
    let n = int_arg(args, 0)?;
    Ok(usize::try_from(n)?)
}

fn insert_index(s: &str, args: &[Value]) -> Result<usize> {
    let idx = byte_index(args)?;
    if !s.is_char_boundary(idx) {
        bail!("assertion failed: self.is_char_boundary(idx)");
    }
    Ok(idx)
}

fn replace_range(s: &mut String, args: &[Value]) -> Result<()> {
    let Value::Range {
        start,
        end,
        inclusive,
    } = arg(args, 0)?
    else {
        bail!("replace_range takes a range");
    };
    let (a, b) = range_bounds(s.len(), start, end, inclusive)?;
    if b > s.len() {
        bail!(
            "range end index {b} out of range for slice of length {}",
            s.len()
        );
    }
    if !s.is_char_boundary(a) {
        bail!("start of range should be a character boundary");
    }
    if !s.is_char_boundary(b) {
        bail!("end of range should be a character boundary");
    }
    s.replace_range(a..b, &arg(args, 1)?.display());
    Ok(())
}

fn remove(s: &mut String, args: &[Value]) -> Result<char> {
    let idx = byte_index(args)?;
    if idx > s.len() {
        bail!(
            "start byte index {idx} is out of bounds for string of length {}",
            s.len()
        );
    }
    if !s.is_char_boundary(idx) {
        return Err(char_boundary_error(s, idx, idx));
    }
    if idx == s.len() {
        bail!("cannot remove a char from the end of a string");
    }
    Ok(s.remove(idx))
}
