//! `?`, casts, pattern tests, formatting and await.

use anyhow::{Result, anyhow, bail};
use num_traits::AsPrimitive;

use super::{Flow, StepCtx};
use crate::interpreter::bytecode::MacroKind;
use crate::interpreter::numeric::{float_to_int, truncate};
use crate::interpreter::ops::{self};
use crate::interpreter::pattern::{bind_pattern_refs, take_bound, try_bind};
use crate::interpreter::typeir::CastIr;
use crate::interpreter::value::Value;

pub(super) fn try_op(ctx: &mut StepCtx, dst: u16, src: u16, conv: u16) -> Result<Flow> {
    Ok(match ops::eval_try(ctx.get(src).clone())? {
        Ok(v) => ctx.set(dst, v),
        Err(early) => {
            ctx.ret = convert_early(ctx, early, conv)?;
            Flow::Ret
        }
    })
}

pub(super) fn try_jump(ctx: &mut StepCtx, dst: u16, src: u16, to: u32, conv: u16) -> Result<Flow> {
    Ok(match ops::eval_try(ctx.get(src).clone())? {
        Ok(v) => {
            ctx.put(dst, v);
            Flow::Jump(to as usize)
        }
        // falls through into the scope drops and the `Ret` after this op
        Err(early) => {
            let early = convert_early(ctx, early, conv)?;
            ctx.set(dst, early)
        }
    })
}

/// The error converts through the `From` impl of the frame type. One already of that type, or one
/// no impl converts, is left as is.
pub(super) fn convert_early(ctx: &StepCtx, early: Value, conv: u16) -> Result<Value> {
    if conv == crate::interpreter::bytecode::NO_CONV {
        return Ok(early);
    }
    let target = &ctx.cur.try_targets[conv as usize];
    let payload = match &early {
        Value::Enum { def, variant, data }
            if def.kind == crate::interpreter::enum_def::EnumKind::Result
                && *variant == crate::interpreter::enum_def::ERR =>
        {
            data.lock().first().cloned()
        }
        _ => None,
    };
    let Some(payload) = payload else {
        return Ok(early);
    };
    let Some(chunk) = ctx.vm.conversion_impl(target, &payload) else {
        return Ok(early);
    };
    let converted = ctx.vm.run_chunk(&chunk, &[payload], &[], true)?;
    Ok(Value::err(converted))
}

pub(super) fn cast_op(ctx: &mut StepCtx, dst: u16, src: u16, ty: u16) -> Result<Flow> {
    let v = eval_cast(&ctx.cur.casts[ty as usize], ctx.get(src).clone())?;
    Ok(ctx.set(dst, v))
}

pub(super) fn coerce_op(ctx: &mut StepCtx, dst: u16, src: u16, ty: u16) -> Flow {
    let v = ctx
        .vm
        .coerce_value(ctx.get(src).clone(), &ctx.cur.coerces[ty as usize]);
    ctx.set(dst, v)
}

pub(super) fn test_bind(ctx: &mut StepCtx, val: u16, pat: u16, dst: u16) -> Flow {
    let info = &ctx.cur.pats[pat as usize];
    let raw = ctx.get(val).clone();
    // the bindings of a reference scrutinee borrow, so `if let Some(v) = &mut opt { v.push(..) }`
    // writes into `opt`
    let (value, by_ref) = match &raw {
        Value::Ref(reference) => match reference.get() {
            Some(inner) => (inner, true),
            None => (Value::Unit, false),
        },
        _ => (raw, false),
    };
    let binds = &info.binds;
    let consts: Vec<Value> = info
        .consts
        .iter()
        .map(|reg| ctx.get(*reg).clone())
        .collect();
    let mut writes: Vec<(u16, Value)> = Vec::new();
    let matched = if by_ref {
        // match first, then anchor each binding to its payload storage
        let matched = try_bind(&info.pat, &value, &consts, &mut |_, _| {});
        if matched {
            let mut define = |name: &str, v: Value| {
                if let Some((_, reg)) = binds.iter().find(|(n, _)| n == name) {
                    writes.push((*reg, v));
                }
            };
            bind_pattern_refs(&info.pat, &value, &consts, &mut define);
        }
        matched
    } else {
        let mut define = |name: &str, v: Value| {
            if let Some((_, reg)) = binds.iter().find(|(n, _)| n == name) {
                writes.push((*reg, v));
            }
        };
        try_bind(&info.pat, &value, &consts, &mut define)
    };
    for (reg, v) in writes {
        ctx.put(reg, v);
    }
    ctx.set(dst, Value::Bool(matched))
}

/// A reference scrutinee lent its parts, so it stays whole.
pub(super) fn take_binds(ctx: &mut StepCtx, val: u16, pat: u16) -> Flow {
    let info = &ctx.cur.pats[pat as usize];
    let value = ctx.get(val).clone();
    if matches!(value, Value::Ref(_)) {
        return Flow::Next;
    }
    let consts: Vec<Value> = info
        .consts
        .iter()
        .map(|reg| ctx.get(*reg).clone())
        .collect();
    if take_bound(&info.pat, &value, &consts) {
        ctx.put(val, Value::Unit);
    }
    Flow::Next
}

pub(super) fn fmt_op(ctx: &mut StepCtx, dst: u16, spec: u16) -> Result<Flow> {
    let text = ctx.vm.render_fmt(ctx.cur, spec, &ctx.stack[ctx.base..])?;
    Ok(ctx.set(dst, Value::str(text)))
}

pub(super) fn macro_call(ctx: &mut StepCtx, kind: MacroKind, dst: u16, spec: u16) -> Result<Flow> {
    let text = ctx.vm.render_fmt(ctx.cur, spec, &ctx.stack[ctx.base..])?;
    Ok(match kind {
        MacroKind::Println => {
            println!("{text}");
            ctx.set(dst, Value::Unit)
        }
        MacroKind::Print => {
            print!("{text}");
            ctx.set(dst, Value::Unit)
        }
        MacroKind::Eprintln => {
            eprintln!("{text}");
            ctx.set(dst, Value::Unit)
        }
        MacroKind::Eprint => {
            eprint!("{text}");
            ctx.set(dst, Value::Unit)
        }
        MacroKind::Panic => bail!("{text}"),
        MacroKind::Anyhow => ctx.set(dst, Value::err(Value::str(text))),
        MacroKind::Bail => {
            ctx.ret = Value::err(Value::str(text));
            Flow::Ret
        }
    })
}

pub(super) fn dbg_op(ctx: &mut StepCtx, dst: u16, first: u16, argc: u16) -> Flow {
    let (first, argc) = (first as usize, argc as usize);
    let mut last = Value::Unit;
    for i in 0..argc {
        last = ctx.stack[ctx.base + first + i].clone();
        eprintln!("[dbg] {}", last.debug());
    }
    ctx.set(dst, last)
}

pub(super) fn await_op(ctx: &mut StepCtx, dst: u16, src: u16) -> Result<Flow> {
    let v = ctx.take(src);
    Ok(ctx.set(dst, ctx.vm.await_value(v)?))
}

pub(super) fn eval_cast(target: &CastIr, v: Value) -> Result<Value> {
    let width = match target {
        CastIr::F64 => {
            return Ok(Value::Float(match v {
                Value::Int(i) => AsPrimitive::<f64>::as_(i),
                Value::IntW(bits, w) => AsPrimitive::<f64>::as_(w.decode(bits)),
                Value::Big(bits, w) => {
                    if w == crate::interpreter::numeric::IntWidth::U128 {
                        AsPrimitive::<f64>::as_(bits.cast_unsigned())
                    } else {
                        AsPrimitive::<f64>::as_(bits)
                    }
                }
                Value::Float(f) => f,
                Value::F32(f) => f64::from(f),
                other => bail!("cannot cast {} to float", other.type_name()),
            }));
        }
        CastIr::F32 => {
            return Ok(Value::F32(match v {
                Value::Int(i) => AsPrimitive::<f32>::as_(i),
                Value::IntW(bits, w) => AsPrimitive::<f32>::as_(w.decode(bits)),
                Value::Float(f) => AsPrimitive::<f32>::as_(f),
                Value::F32(f) => f,
                other => bail!("cannot cast {} to float", other.type_name()),
            }));
        }
        CastIr::Char => {
            return Ok(match v {
                Value::Int(_) | Value::IntW(..) => {
                    let i = v.int_parts().map_or(0, |(value, _)| value);
                    Value::Char(
                        u32::try_from(i)
                            .ok()
                            .and_then(char::from_u32)
                            .ok_or_else(|| anyhow!("invalid char code {i}"))?,
                    )
                }
                Value::Char(c) => Value::Char(c),
                other => bail!("cannot cast {} to char", other.type_name()),
            });
        }
        CastIr::Unsupported(name) => bail!("unsupported cast target: {name}"),
        CastIr::Int(width) => *width,
    };
    let value = match v {
        Value::Int(i) => truncate(i128::from(i), width),
        Value::IntW(bits, w) => truncate(w.decode(bits), width),
        // the stored i128 has the exact bits, so a narrowing cast keeps the low bits
        Value::Big(bits, _) => truncate(bits, width),
        Value::Float(f) => float_to_int(f, width),
        Value::F32(f) => float_to_int(f64::from(f), width),
        Value::Char(c) => truncate(i128::from(c as u32), width),
        Value::Bool(b) => i128::from(b),
        other => bail!("cannot cast {} to integer", other.type_name()),
    };
    Ok(Value::int_of_width(value, width))
}
