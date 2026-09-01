//! The `Method` and `GetOrDefault` op bodies. Builtins with an in place form are handled here by
//! id, the rest goes to the generic dispatch.

use std::mem::take;

use anyhow::{Result, bail};

use super::bytecode::{BuiltinId, MethodName};
use super::enum_def::{EnumKind, OK, SOME};
use super::methods::{make_ordering, str_grow};
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
    if let Some(v) = builtin_fast(ctx, recv, name, s, argc, dst) {
        return Ok(ctx.set_opt(dst, v));
    }
    // fallible in place forms, the closure can error
    if let BuiltinId::GetOrInsert | BuiltinId::GetOrInsertWith = name.id
        && argc == 1
        && is_option(ctx, recv)
    {
        let v = option_get_or_insert(ctx, recv, name.id, s)?;
        return Ok(ctx.set_opt(dst, v));
    }
    // an integer method with integer arguments skips the whole dispatch walk
    if matches!(ctx.stack[base + recv], Value::Int(_) | Value::IntW(..))
        && ctx.stack[s..s + argc]
            .iter()
            .all(|arg| matches!(arg, Value::Int(_) | Value::IntW(..)))
        && let Some(result) = crate::interpreter::bridge::int_method(
            &ctx.stack[base + recv],
            name,
            &ctx.stack[s..s + argc],
        )
    {
        return Ok(ctx.set_opt(dst, result?));
    }
    // The arg window holds dead temporaries, so a `read_line(&mut s)` buffer lands back in its
    // register this way.
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

/// A call whose guard fails returns `None` and takes the generic path. The mutating arms write the
/// receiver register directly, the generic path hands the method a clone and would lose the change.
fn builtin_fast(
    ctx: &mut StepCtx,
    recv: usize,
    name: &MethodName,
    s: usize,
    argc: usize,
    dst: u16,
) -> Option<Value> {
    let base = ctx.base;
    match name.id {
        BuiltinId::CloneFrom if argc == 1 => Some(clone_from(ctx, recv, s)),
        BuiltinId::Take if argc == 0 && is_option(ctx, recv) => Some(option_take(ctx, recv)),
        BuiltinId::Replace if argc == 1 && is_option(ctx, recv) => {
            Some(option_replace(ctx, recv, s))
        }
        BuiltinId::MakeAsciiUppercase => ascii_case(ctx, recv, true),
        BuiltinId::MakeAsciiLowercase => ascii_case(ctx, recv, false),
        BuiltinId::Push | BuiltinId::PushStr
            if argc == 1 && matches!(ctx.stack[base + recv], Value::Str(_)) =>
        {
            str_push(ctx, recv, name.id, s)
        }
        // `write!` into a `String` is `fmt::Write`, the result is `fmt::Result`
        BuiltinId::WriteAll | BuiltinId::WriteStr | BuiltinId::WriteFmt
            if argc == 1 && matches!(ctx.stack[base + recv], Value::Str(_)) =>
        {
            str_push(ctx, recv, BuiltinId::PushStr, s).map(|_| Value::ok(Value::Unit))
        }
        // off a place `clear` is the colored crate's one, which returns a value
        BuiltinId::Clear
            if argc == 0 && name.place && matches!(ctx.stack[base + recv], Value::Str(_)) =>
        {
            ctx.stack[base + recv] = Value::str(String::new());
            Some(Value::Unit)
        }
        // skipped when the script defines methods that could shadow these
        BuiltinId::Copied | BuiltinId::Cloned | BuiltinId::Unwrap | BuiltinId::UnwrapOr
            if ctx.vm.impls.is_empty() =>
        {
            option_fast(ctx, recv, name.id, s, argc)
        }
        // a refcount bump, not worth the dispatch walk
        BuiltinId::ToString | BuiltinId::Clone => match &ctx.stack[base + recv] {
            Value::Str(v) => Some(Value::Str(v.clone())),
            _ => None,
        },
        BuiltinId::Cmp if argc == 1 => int_cmp(ctx, recv, s),
        BuiltinId::Get | BuiltinId::Insert | BuiltinId::ContainsKey => {
            map_fast(ctx, recv, name.id, s, argc, dst)
        }
        _ => None,
    }
}

fn is_option(ctx: &StepCtx, recv: usize) -> bool {
    ctx.stack[ctx.base + recv].is_enum_kind(EnumKind::Option)
}

fn clone_from(ctx: &mut StepCtx, recv: usize, s: usize) -> Value {
    let src = ctx.stack[s].clone();
    ctx.stack[ctx.base + recv] = src;
    Value::Unit
}

/// `RefCell::take` and an iterator `take(n)` are kept out by the guard.
/// Fills a `None` receiver in place, then hands out a reference into the payload slot, so a
/// `push` through the returned binding lands in the option.
fn option_get_or_insert(ctx: &mut StepCtx, recv: usize, id: BuiltinId, s: usize) -> Result<Value> {
    let slot = ctx.base + recv;
    let is_none = matches!(
        &ctx.stack[slot],
        Value::Enum { variant, .. } if *variant == super::enum_def::NONE
    );
    if is_none {
        let filler = ctx.stack[s].clone();
        let value = if id == BuiltinId::GetOrInsert {
            filler
        } else {
            let Value::Closure(clo) = filler else {
                bail!("get_or_insert_with takes a function");
            };
            ctx.vm.call_closure_data(&clo, &[])?
        };
        ctx.stack[slot] = Value::some(value);
    }
    match &ctx.stack[slot] {
        Value::Enum { data, .. } => Ok(Value::Ref(std::sync::Arc::new(
            super::value::ValueRef::vec_element(data.clone(), 0),
        ))),
        _ => bail!("get_or_insert needs an Option receiver"),
    }
}

fn option_take(ctx: &mut StepCtx, recv: usize) -> Value {
    let old = take(&mut ctx.stack[ctx.base + recv]);
    ctx.stack[ctx.base + recv] = Value::none();
    old
}

fn option_replace(ctx: &mut StepCtx, recv: usize, s: usize) -> Value {
    let new = ctx.stack[s].clone();
    let old = take(&mut ctx.stack[ctx.base + recv]);
    ctx.stack[ctx.base + recv] = Value::some(new);
    old
}

/// Other receivers give `None` and take the generic path.
fn ascii_case(ctx: &mut StepCtx, recv: usize, upper: bool) -> Option<Value> {
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
        _ => return None,
    };
    *slot = new;
    Some(Value::Unit)
}

/// The argument is cloned out first, a refcount bump that also keeps `s.push_str(&s)` sound.
fn str_push(ctx: &mut StepCtx, recv: usize, id: BuiltinId, s: usize) -> Option<Value> {
    let arg = ctx.stack[s].clone();
    let Value::Str(text) = &mut ctx.stack[ctx.base + recv] else {
        return None;
    };
    str_grow(text, id, &arg).ok()?;
    Some(Value::Unit)
}

/// A comparator sort calls `cmp` per comparison, so the int form skips the bridge walk.
fn int_cmp(ctx: &StepCtx, recv: usize, s: usize) -> Option<Value> {
    if let Value::Int(a) = ctx.stack[ctx.base + recv]
        && let Value::Int(b) = ctx.stack[s]
    {
        return Some(make_ordering(a.cmp(&b)));
    }
    None
}

/// These dominate counting loops, so the success paths run here. Failure paths get their errors
/// from the slow path.
fn option_fast(
    ctx: &mut StepCtx,
    recv: usize,
    id: BuiltinId,
    s: usize,
    argc: usize,
) -> Option<Value> {
    let (kind, success) = match &ctx.stack[ctx.base + recv] {
        Value::Enum { def, variant, .. } => match def.kind {
            EnumKind::Option => (EnumKind::Option, *variant == SOME),
            EnumKind::Result => (EnumKind::Result, *variant == OK),
            _ => return None,
        },
        _ => return None,
    };
    match id {
        // `copied` and `cloned` return the Option itself
        BuiltinId::Copied | BuiltinId::Cloned if kind == EnumKind::Option => {
            Some(ctx.stack[ctx.base + recv].deep_clone())
        }
        BuiltinId::Unwrap | BuiltinId::UnwrapOr if success => {
            let Value::Enum { data, .. } = &ctx.stack[ctx.base + recv] else {
                return None;
            };
            data.lock().first().cloned()
        }
        BuiltinId::UnwrapOr if argc == 1 => Some(take(&mut ctx.stack[s])),
        _ => None,
    }
}

/// Same reason as the Option accessors. User methods can't exist on a `HashMap`, so no gate is needed.
fn map_fast(
    ctx: &mut StepCtx,
    recv: usize,
    id: BuiltinId,
    s: usize,
    argc: usize,
    dst: u16,
) -> Option<Value> {
    let base = ctx.base;
    if !matches!(ctx.stack[base + recv], Value::Map(_, MapKind::Map))
        || argc < 1
        || base + recv >= s
    {
        return None;
    }
    let (lo, hi) = ctx.stack.split_at_mut(s);
    let Value::Map(m, _) = &lo[base + recv] else {
        unreachable!()
    };
    let k = hi[0].as_key()?;
    Some(if id == BuiltinId::Insert {
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
    } else if id == BuiltinId::ContainsKey {
        Value::Bool(m.lock().get(&k).is_some())
    } else {
        match m.lock().get(&k).cloned() {
            Some(v) => Value::some(v),
            None => Value::none(),
        }
    })
}

pub(super) fn get_or_default(
    ctx: &mut StepCtx,
    dst: u16,
    recv: u16,
    key: u16,
    default: u16,
) -> Result<Flow> {
    let recv_v = ctx.get(recv).clone();
    let key_v = ctx.get(key).clone();
    let get = MethodName::builtin(BuiltinId::Get);
    let opt = ctx.vm.eval_method(&recv_v, &get, &mut [key_v])?;
    let v = opt
        .some_payload()
        .unwrap_or_else(|| ctx.get(default).clone());
    Ok(ctx.set(dst, v))
}
