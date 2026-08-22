//! The place ops, derefs, field and index writes and element references.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use super::{Flow, StepCtx, user_bin};
use crate::interpreter::bytecode::Member;
use crate::interpreter::ops::{self, apply_bin, int_of};
use crate::interpreter::value::Value;
use crate::interpreter::vm::Vm;

/// A reference base resolves to its referent and a shared pointer auto derefs.
pub(super) fn place_base(v: &Value) -> Result<Value> {
    Ok(match v {
        Value::Ref(reference) => reference
            .get()
            .ok_or_else(|| anyhow!("access through a dangling reference"))?,
        Value::Cell(_, slot) => slot.lock().clone(),
        other => other.clone(),
    })
}

pub(super) fn set_index(ctx: &mut StepCtx, base: u16, key: u16, val: u16) -> Result<Flow> {
    // A range write into a string is the writeback of `s[2..].make_ascii_uppercase()`, stored
    // through the base itself.
    if let &Value::Range {
        start,
        end,
        inclusive,
    } = ctx.get(key)
    {
        let target = place_base(ctx.get(base))?;
        if let Value::Str(s) = &target {
            let new = Value::str(ops::splice_str(s, start, end, inclusive, ctx.get(val))?);
            let flow = match ctx.get(base).clone() {
                Value::Cell(_, slot) => {
                    *slot.lock() = new;
                    Flow::Next
                }
                Value::Ref(reference) => {
                    if !reference.set(new) {
                        bail!("assignment through a dangling reference");
                    }
                    Flow::Next
                }
                _ => ctx.set(base, new),
            };
            return Ok(flow);
        }
    }
    let target = place_base(ctx.get(base))?;
    ops::set_index(&target, ctx.get(key), ctx.get(val).clone())?;
    Ok(Flow::Next)
}

pub(super) fn deref_op(ctx: &mut StepCtx, dst: u16, src: u16) -> Result<Flow> {
    let v = deref(ctx.get(src))?;
    Ok(ctx.set(dst, v))
}

pub(super) fn deref(v: &Value) -> Result<Value> {
    Ok(match v {
        Value::Ref(reference) => reference
            .get()
            .ok_or_else(|| anyhow!("dereference of a dangling reference"))?,
        // `*rc` reads the content
        Value::Cell(_, slot) => slot.lock().clone(),
        value => value.clone(),
    })
}

pub(super) fn set_deref(ctx: &StepCtx, target: u16, val: u16) -> Result<Flow> {
    let Value::Ref(reference) = ctx.get(target) else {
        bail!("assignment through a non-reference value");
    };
    if !reference.set(ctx.get(val).clone()) {
        bail!("assignment through a dangling reference");
    }
    Ok(Flow::Next)
}

/// A value a fused compound assignment may touch under the held lock. `apply_bin` on these takes
/// no lock and runs no user code.
pub(super) fn fusable_scalar(v: &Value) -> bool {
    matches!(
        v,
        Value::Int(_) | Value::IntW(..) | Value::Float(_) | Value::F32(_) | Value::Bool(_)
    )
}

/// `*r op= v` as 1 op. Plain scalars run under the referent's lock so concurrent tasks can't lose
/// updates. Everything else runs the unfused sequence, errors included.
pub(super) fn deref_bin_assign(
    ctx: &mut StepCtx,
    target: u16,
    val: u16,
    op: crate::interpreter::bytecode::BinKind,
) -> Result<Flow> {
    if let Value::Ref(reference) = ctx.get(target)
        && fusable_scalar(ctx.get(val))
    {
        let reference = reference.clone();
        let b = ctx.get(val).clone();
        let fused = reference.update(|current| {
            if !fusable_scalar(current) {
                return Ok(false);
            }
            *current = apply_bin(op, current, &b)?;
            Ok(true)
        });
        match fused {
            Some(Ok(true)) => return Ok(Flow::Next),
            Some(Err(e)) => return Err(e),
            Some(Ok(false)) | None => {}
        }
    }
    let current = deref(ctx.get(target))?;
    let b = ctx.get(val).clone();
    let result = match user_bin(ctx, op, &current, &b)? {
        Some(v) => v,
        None => apply_bin(op, &current, &b)?,
    };
    let Value::Ref(reference) = ctx.get(target) else {
        bail!("assignment through a non-reference value");
    };
    if !reference.set(result) {
        bail!("assignment through a dangling reference");
    }
    Ok(Flow::Next)
}

/// A real reference is set through, a plain value is written into the parameter register for the
/// caller's writeback.
pub(super) fn set_deref_param(ctx: &mut StepCtx, target: u16, val: u16) -> Result<Flow> {
    if let Value::Ref(reference) = ctx.get(target) {
        if !reference.set(ctx.get(val).clone()) {
            bail!("assignment through a dangling reference");
        }
        return Ok(Flow::Next);
    }
    let value = ctx.get(val).clone();
    Ok(ctx.set(target, value))
}

pub(super) fn get_field_op(ctx: &mut StepCtx, dst: u16, base: u16, member: u16) -> Result<Flow> {
    let target = place_base(ctx.get(base))?;
    let v = Vm::get_field(&target, &ctx.cur.members[member as usize])?;
    Ok(ctx.set(dst, v))
}

pub(super) fn set_field_op(ctx: &StepCtx, base: u16, member: u16, val: u16) -> Result<Flow> {
    let target = place_base(ctx.get(base))?;
    Vm::set_field(
        &target,
        &ctx.cur.members[member as usize],
        ctx.get(val).clone(),
    )?;
    Ok(Flow::Next)
}

pub(super) fn ref_index(ctx: &mut StepCtx, dst: u16, base: u16, key: u16) -> Result<Flow> {
    let target = place_base(ctx.get(base))?;
    let v = match (&target, ctx.get(key)) {
        (Value::Vec(list), key_val) => {
            let i = usize::try_from(int_of(key_val)?)?;
            let len = list.lock().len();
            if i >= len {
                bail!("index out of bounds: the len is {len} but the index is {i}");
            }
            Value::Ref(Arc::new(crate::interpreter::value::ValueRef::vec_element(
                list.clone(),
                i,
            )))
        }
        (Value::Map(map, _), key_val) => {
            let k = key_val.as_key().ok_or_else(|| anyhow!("invalid map key"))?;
            Value::Ref(Arc::new(crate::interpreter::value::ValueRef::map_entry(
                map.clone(),
                k,
            )))
        }
        (recv, _) => bail!("cannot take `&mut` of an element of {}", recv.type_name()),
    };
    Ok(ctx.set(dst, v))
}

/// A tuple field borrows as a list element.
pub(super) fn ref_field(ctx: &mut StepCtx, dst: u16, base: u16, member: u16) -> Result<Flow> {
    let member = &ctx.cur.members[member as usize];
    let target = place_base(ctx.get(base))?;
    let v = match (&target, member) {
        (Value::Struct(s), Member::Named(n)) => {
            let Some(slot) = n.slot_in(&s.shape) else {
                bail!("no field `{n}`");
            };
            Value::Ref(Arc::new(crate::interpreter::value::ValueRef::struct_field(
                s.clone(),
                slot,
            )))
        }
        (Value::Struct(s), Member::Indexed(i)) => Value::Ref(Arc::new(
            crate::interpreter::value::ValueRef::struct_field(s.clone(), *i),
        )),
        (Value::Tuple(t), Member::Indexed(i)) => Value::Ref(Arc::new(
            crate::interpreter::value::ValueRef::vec_element(t.clone(), *i),
        )),
        (recv, _) => bail!("cannot take `&mut` of a field of {}", recv.type_name()),
    };
    Ok(ctx.set(dst, v))
}
