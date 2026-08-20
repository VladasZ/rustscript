//! The `Method` and `GetOrDefault` op bodies. A builtin with an in place
//! form is answered here by one `match` on its id, everything else goes to
//! the generic method dispatch.

use std::mem::take;

use anyhow::Result;

use super::bytecode::{BuiltinId, MethodName};
use super::enum_def::{EnumKind, OK, SOME};
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
    if let Some(v) = builtin_fast(ctx, recv, name, s, argc, dst) {
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

/// The builtins answered without the dispatch walk, keyed by id. Each arm
/// states its own receiver and arity conditions in its guard. A call whose
/// guard fails, or whose arm finds an argument it cannot use, answers `None`
/// and takes the generic path, which reports any error. The arms that mutate
/// the receiver write its register directly, the generic path hands the
/// method a clone and would lose the change. A place receiver, a slice
/// element, a cell, or a field, lands through the writeback the compiler
/// emits after the op.
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
        BuiltinId::CloneFrom => Some(clone_from(ctx, recv, s, argc)),
        BuiltinId::Take if argc == 0 && is_option(ctx, recv) => Some(option_take(ctx, recv)),
        BuiltinId::Replace if argc == 1 && is_option(ctx, recv) => {
            Some(option_replace(ctx, recv, s))
        }
        BuiltinId::MakeAsciiUppercase => ascii_case(ctx, recv, true),
        BuiltinId::MakeAsciiLowercase => ascii_case(ctx, recv, false),
        BuiltinId::Push | BuiltinId::PushStr if matches!(ctx.stack[base + recv], Value::Str(_)) => {
            Some(str_push(ctx, recv, name.id, s, argc))
        }
        // Skipped when the script defines methods, which could shadow these.
        BuiltinId::Copied | BuiltinId::Cloned | BuiltinId::Unwrap | BuiltinId::UnwrapOr
            if ctx.vm.impls.is_empty() =>
        {
            option_fast(ctx, recv, name.id, s, argc)
        }
        // to_string and clone on a string are a refcount bump, not worth the
        // dispatch walk.
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

fn first_arg(ctx: &StepCtx, s: usize, argc: usize) -> Value {
    ctx.stack[s..s + argc]
        .first()
        .cloned()
        .unwrap_or(Value::Unit)
}

/// `clone_from` replaces the receiver outright.
fn clone_from(ctx: &mut StepCtx, recv: usize, s: usize, argc: usize) -> Value {
    let src = first_arg(ctx, s, argc);
    ctx.stack[ctx.base + recv] = src;
    Value::Unit
}

/// `Option::take` on the receiver place. A `RefCell::take` is the cell
/// bridge's and an iterator `take(n)` is an adaptor, the guard keeps both
/// out.
fn option_take(ctx: &mut StepCtx, recv: usize) -> Value {
    let old = take(&mut ctx.stack[ctx.base + recv]);
    ctx.stack[ctx.base + recv] = Value::none();
    old
}

fn option_replace(ctx: &mut StepCtx, recv: usize, s: usize) -> Value {
    let new = first_arg(ctx, s, 1);
    let old = take(&mut ctx.stack[ctx.base + recv]);
    ctx.stack[ctx.base + recv] = Value::some(new);
    old
}

/// Rewrite a string or char receiver register through the ascii casing
/// methods. Other receivers answer `None` and take the generic path.
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

/// `push` and `push_str` on a string receiver grow it in place. The argument
/// is cloned out first so the receiver can be borrowed mutably. A string
/// clone is a refcount bump, and it also keeps `s.push_str(&s)` sound: the
/// snapshot survives the in-place append.
fn str_push(ctx: &mut StepCtx, recv: usize, id: BuiltinId, s: usize, argc: usize) -> Value {
    let arg = first_arg(ctx, s, argc);
    if let Value::Str(text) = &mut ctx.stack[ctx.base + recv] {
        match (id, &arg) {
            (BuiltinId::Push, Value::Char(c)) => text.push(*c),
            (BuiltinId::PushStr, Value::Str(other)) => text.push_str(other),
            (BuiltinId::PushStr, other) => text.push_str(&other.display()),
            _ => {}
        }
    }
    Value::Unit
}

/// A comparator sort calls `cmp` once per comparison, so the plain int
/// form answers here without the bridge walk or its argument decode.
fn int_cmp(ctx: &StepCtx, recv: usize, s: usize) -> Option<Value> {
    if let Value::Int(a) = ctx.stack[ctx.base + recv]
        && let Value::Int(b) = ctx.stack[s]
    {
        return Some(make_ordering(a.cmp(&b)));
    }
    None
}

/// Option and Result accessors dominate counting loops, so their success
/// paths run right here, skipping the whole dispatch chain. Failure paths
/// fall through and get their errors from the slow path.
fn option_fast(
    ctx: &mut StepCtx,
    recv: usize,
    id: BuiltinId,
    s: usize,
    argc: usize,
) -> Option<Value> {
    // 0 none, 1 clone receiver, 2 clone payload, 3 default
    let choice = match &ctx.stack[ctx.base + recv] {
        Value::Enum { def, variant, .. } => {
            let success = match def.kind {
                EnumKind::Option => Some(*variant == SOME),
                EnumKind::Result => Some(*variant == OK),
                _ => None,
            };
            if matches!(id, BuiltinId::Copied | BuiltinId::Cloned) {
                i32::from(def.kind == EnumKind::Option)
            } else if success == Some(true) {
                2
            } else if success.is_none() {
                0
            } else if id == BuiltinId::UnwrapOr {
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
/// User methods cannot exist on a `HashMap`, so no gate is needed. A key
/// that is not a valid map key answers `None` untouched, and the generic
/// path reports it.
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
    let get = MethodName::builtin(BuiltinId::Get);
    let opt = ctx.vm.eval_method(&recv_v, &get, &mut [key_v])?;
    let v = opt
        .some_payload()
        .unwrap_or_else(|| ctx.get(default).clone());
    Ok(ctx.set(dst, v))
}
