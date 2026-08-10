//! The `Method` and `GetOrDefault` op bodies: the builtin fast paths for
//! strings, options, and maps, then the generic method dispatch.

use std::mem::take;

use anyhow::{Result, bail};

use super::bytecode::{BuiltinId, MethodName};
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
    // A string is an immutable `Arc<str>`, so a push has to rewrite the
    // receiver register itself. The normal path hands the method a clone and
    // the change would be lost. `clone_from` replaces the receiver outright,
    // so it has to write the register rather than a copy of it.
    if name.id == BuiltinId::CloneFrom {
        let src = ctx.stack[s..s + argc]
            .first()
            .cloned()
            .unwrap_or(Value::Unit);
        ctx.stack[base + recv] = src;
        return Ok(ctx.set_opt(dst, Value::Unit));
    }
    if matches!(name.id, BuiltinId::Push | BuiltinId::PushStr)
        && let Value::Str(text) = &ctx.stack[base + recv]
    {
        let mut out = text.to_string();
        match (&name.id, ctx.stack[s..s + argc].first()) {
            (BuiltinId::Push, Some(Value::Char(c))) => out.push(*c),
            (BuiltinId::PushStr, Some(arg)) => out.push_str(&arg.display()),
            _ => {}
        }
        ctx.stack[base + recv] = Value::str(out);
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
            Value::Enum { data, .. } => Some(data.first().cloned().unwrap_or(Value::Unit)),
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
            data.first().cloned().unwrap_or(Value::Unit)
        }
        _ => ctx.get(default).clone(),
    };
    Ok(ctx.set(dst, v))
}
