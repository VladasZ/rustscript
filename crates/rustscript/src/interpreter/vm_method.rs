//! The `Method` and `GetOrDefault` op bodies: the builtin fast paths for
//! strings, options, and maps, then the generic method dispatch.

use std::mem::take;

use anyhow::{Result, bail};

use super::bytecode::{BuiltinId, MethodName};
use super::methods::make_ordering;
use super::value::{MapKind, Value};
use super::vm_step::{Flow, StepCtx};

pub(super) fn method_op(
    ctx: &mut StepCtx,
    dst: u16,
    recv: u16,
    name: u16,
    abase: u16,
    argc: u16,
) -> Result<Flow> {
    let (vm, cur, base) = (ctx.vm, ctx.cur, ctx.base);
    let (recv, abase, argc) = (recv as usize, abase as usize, argc as usize);
    let name = &cur.names[name as usize];
    let s = base + abase;
    // A push mutates the receiver register itself. The normal path hands the
    // method a clone and the change would be lost. `clone_from` replaces the
    // receiver outright, so it has to write the register rather than a copy
    // of it.
    if name.id == BuiltinId::CloneFrom {
        let src = ctx.stack[s..s + argc]
            .first()
            .cloned()
            .unwrap_or(Value::Unit);
        ctx.stack[base + recv] = src;
        return Ok(ctx.set_opt(dst, Value::Unit));
    }
    // `Option::take` and `Option::replace` mutate their receiver place. The
    // receiver register is the place, or a place-loaded copy whose writeback
    // the compiler emits after this op, so writing the register is enough.
    // Every other receiver falls through, a `RefCell::take` is the cell
    // bridge's and an iterator `take(n)` is an adaptor.
    if matches!(name.id, BuiltinId::Take)
        && argc == 0
        && matches!(&ctx.stack[base + recv], Value::Enum { enum_name, .. } if &**enum_name == "Option")
    {
        let old = take(&mut ctx.stack[base + recv]);
        ctx.stack[base + recv] = Value::none();
        return Ok(ctx.set_opt(dst, old));
    }
    if name.text == "replace"
        && argc == 1
        && matches!(&ctx.stack[base + recv], Value::Enum { enum_name, .. } if &**enum_name == "Option")
    {
        let new = ctx.stack[s..s + argc]
            .first()
            .cloned()
            .unwrap_or(Value::Unit);
        let old = take(&mut ctx.stack[base + recv]);
        ctx.stack[base + recv] = Value::some(new);
        return Ok(ctx.set_opt(dst, old));
    }
    // In-place ascii casing mutates the receiver register and returns unit,
    // like a push. A slice, cell, or field receiver lands through the place
    // writeback the compiler emits after this op.
    if matches!(&*name.text, "make_ascii_uppercase" | "make_ascii_lowercase")
        && ascii_case_fast(ctx, recv, name)
    {
        return Ok(ctx.set_opt(dst, Value::Unit));
    }
    if matches!(name.id, BuiltinId::Push | BuiltinId::PushStr)
        && matches!(ctx.stack[base + recv], Value::Str(_))
    {
        // The argument is cloned out first so the receiver can be borrowed
        // mutably. A string clone is a refcount bump, and it also keeps
        // `s.push_str(&s)` sound: the snapshot survives the in-place append.
        let arg = ctx.stack[s..s + argc]
            .first()
            .cloned()
            .unwrap_or(Value::Unit);
        if let Value::Str(text) = &mut ctx.stack[base + recv] {
            match (&name.id, &arg) {
                (BuiltinId::Push, Value::Char(c)) => text.push(*c),
                (BuiltinId::PushStr, Value::Str(other)) => text.push_str(other),
                (BuiltinId::PushStr, other) => text.push_str(&other.display()),
                _ => {}
            }
        }
        return Ok(ctx.set_opt(dst, Value::Unit));
    }
    if vm.methods.is_empty()
        && matches!(
            name.id,
            BuiltinId::Copied | BuiltinId::Unwrap | BuiltinId::UnwrapOr
        )
        && let Some(v) = option_fast(ctx, recv, name, s, argc)
    {
        return Ok(ctx.set_opt(dst, v));
    }
    // to_string and clone on a string are a refcount bump, not worth the
    // dispatch walk.
    if matches!(name.id, BuiltinId::ToString | BuiltinId::Clone)
        && let Value::Str(v) = &ctx.stack[base + recv]
    {
        let v = Value::Str(v.clone());
        return Ok(ctx.set_opt(dst, v));
    }
    if let Some(v) = int_cmp_fast(ctx, recv, name, s, argc) {
        return Ok(ctx.set_opt(dst, v));
    }
    if let Some(v) = map_fast(ctx, recv, name, s, argc, dst)? {
        return Ok(ctx.set_opt(dst, v));
    }
    // The arg window holds dead temporaries, so methods may consume or mutate
    // them in place. A `read_line(&mut s)` buffer lands back in its register
    // this way.
    let v = if argc == 0 {
        vm.eval_method(&ctx.stack[base + recv].clone(), name, &mut [])?
    } else if base + recv < s {
        let (lo, hi) = ctx.stack.split_at_mut(s);
        vm.eval_method(&lo[base + recv], name, &mut hi[..argc])?
    } else {
        let recv_v = ctx.stack[base + recv].clone();
        vm.eval_method(&recv_v, name, &mut ctx.stack[s..s + argc])?
    };
    Ok(ctx.set_opt(dst, v))
}

/// Rewrite a string or char receiver register through the ascii casing
/// methods. Answers false for other receivers, which fall to plain dispatch.
fn ascii_case_fast(ctx: &mut StepCtx, recv: usize, name: &MethodName) -> bool {
    let upper = &*name.text == "make_ascii_uppercase";
    let slot = &mut ctx.stack[ctx.base + recv];
    let new = match &*slot {
        Value::Str(text) => Value::str(if upper {
            text.to_ascii_uppercase()
        } else {
            text.to_ascii_lowercase()
        }),
        Value::Char(c) => Value::Char(if upper {
            c.to_ascii_uppercase()
        } else {
            c.to_ascii_lowercase()
        }),
        _ => return false,
    };
    *slot = new;
    true
}

/// A comparator sort calls `cmp` once per comparison, so the plain int
/// form answers here without the bridge walk or its argument decode.
fn int_cmp_fast(
    ctx: &StepCtx,
    recv: usize,
    name: &MethodName,
    s: usize,
    argc: usize,
) -> Option<Value> {
    if argc == 1
        && name.text == "cmp"
        && let Value::Int(a) = ctx.stack[ctx.base + recv]
        && let Value::Int(b) = ctx.stack[s]
    {
        return Some(make_ordering(a.cmp(&b)));
    }
    None
}

/// Option and Result accessors dominate counting loops, so their success
/// paths run right here, skipping the whole dispatch chain. Failure paths
/// fall through and get their errors from the slow path. Skipped when the
/// script defines methods, which could shadow these.
fn option_fast(
    ctx: &mut StepCtx,
    recv: usize,
    name: &MethodName,
    s: usize,
    argc: usize,
) -> Option<Value> {
    // 0 none, 1 clone receiver, 2 clone payload, 3 default
    let choice = match &ctx.stack[ctx.base + recv] {
        Value::Enum {
            enum_name, variant, ..
        } => {
            if matches!(name.id, BuiltinId::Copied) {
                i32::from(&**enum_name == "Option")
            } else if !matches!(&**enum_name, "Option" | "Result") {
                0
            } else if matches!(&**variant, "Some" | "Ok") {
                2
            } else if matches!(name.id, BuiltinId::UnwrapOr) {
                3
            } else {
                0
            }
        }
        _ => 0,
    };
    match choice {
        1 => Some(ctx.stack[ctx.base + recv].clone()),
        2 => match &ctx.stack[ctx.base + recv] {
            Value::Enum { data, .. } => Some(data.lock().first().cloned().unwrap_or(Value::Unit)),
            _ => unreachable!(),
        },
        3 => Some(if argc > 0 {
            take(&mut ctx.stack[s])
        } else {
            Value::Unit
        }),
        _ => None,
    }
}

/// Map get and insert run inline for the same reason as the Option accessors.
/// User methods cannot exist on a `HashMap`, so no gate is needed.
fn map_fast(
    ctx: &mut StepCtx,
    recv: usize,
    name: &MethodName,
    s: usize,
    argc: usize,
    dst: u16,
) -> Result<Option<Value>> {
    let base = ctx.base;
    if !matches!(
        name.id,
        BuiltinId::Get | BuiltinId::Insert | BuiltinId::ContainsKey
    ) || !matches!(ctx.stack[base + recv], Value::Map(_, MapKind::Map))
        || argc < 1
        || base + recv >= s
        // `get_mut` shares the Get id but answers a reference, which only
        // the slow path builds.
        || name.text == "get_mut"
    {
        return Ok(None);
    }
    let (lo, hi) = ctx.stack.split_at_mut(s);
    let Value::Map(m, _) = &lo[base + recv] else {
        unreachable!()
    };
    let v = if name.id == BuiltinId::Insert {
        let Some(k) = take(&mut hi[0]).into_key() else {
            bail!("invalid map key")
        };
        let val = if argc > 1 {
            take(&mut hi[1])
        } else {
            Value::Unit
        };
        let old = m.lock().insert(k, val);
        if dst == u16::MAX {
            Value::Unit
        } else {
            match old {
                Some(old) => Value::some(old),
                None => Value::none(),
            }
        }
    } else {
        let Some(k) = hi[0].as_key() else {
            bail!("invalid map key")
        };
        if matches!(name.id, BuiltinId::ContainsKey) {
            Value::Bool(m.lock().get(&k).is_some())
        } else {
            match m.lock().get(&k).cloned() {
                Some(v) => Value::some(v),
                None => Value::none(),
            }
        }
    };
    Ok(Some(v))
}

/// Fused `recv.get(key).copied().unwrap_or(default)`.
pub(super) fn get_or_default(
    ctx: &mut StepCtx,
    dst: u16,
    recv: u16,
    key: u16,
    default: u16,
) -> Result<Flow> {
    let recv_v = ctx.get(recv).clone();
    let key_v = ctx.get(key).clone();
    let get = MethodName {
        text: "get".into(),
        id: BuiltinId::Get,
        scalar: None,
    };
    let opt = ctx.vm.eval_method(&recv_v, &get, &mut [key_v])?;
    let v = match opt {
        Value::Enum { variant, data, .. } if &*variant == "Some" => {
            data.lock().first().cloned().unwrap_or(Value::Unit)
        }
        _ => ctx.get(default).clone(),
    };
    Ok(ctx.set(dst, v))
}
