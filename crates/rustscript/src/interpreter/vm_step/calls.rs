//! The call, constructor and closure ops.

use std::iter::repeat_n;
use std::mem::take;
use std::sync::Arc;

use anyhow::{Result, bail};
use parking_lot::Mutex;

use super::{CallReq, Flow, StepCtx};
use crate::interpreter::bytecode::{CapSource, Chunk, PathId, path_call_chunk};
use crate::interpreter::iterator::FastNext;
use crate::interpreter::native::Native;
use crate::interpreter::ops::int_of;
use crate::interpreter::value::{ClosureData, StructShape, Upvalue, Value};
use crate::interpreter::vm::{TypeEnv, empty_type_env};

pub(super) fn call_fn(
    ctx: &mut StepCtx,
    dst: u16,
    func: u32,
    abase: u16,
    argc: u16,
    targ: u32,
) -> Result<Flow> {
    let callee = ctx.vm.functions[func as usize].clone();
    let type_env: TypeEnv = if targ == u32::MAX {
        empty_type_env()
    } else {
        let targs = &ctx.cur.call_type_args[targ as usize];
        callee
            .generics
            .iter()
            .zip(targs.iter())
            .map(|(name, ty)| (name.clone(), ty.clone()))
            .collect()
    };
    request_call(ctx, callee, None, dst, abase, argc, type_env)
}

pub(super) fn call_value(
    ctx: &mut StepCtx,
    dst: u16,
    callee: u16,
    abase: u16,
    argc: u16,
) -> Result<Flow> {
    let clo = match ctx.get(callee) {
        Value::Closure(clo) => clo.clone(),
        other => bail!("cannot call {}", other.type_name()),
    };
    let chunk = clo.chunk.clone();
    request_call(ctx, chunk, Some(clo), dst, abase, argc, empty_type_env())
}

/// The arg count is checked here, where the error can name the callee.
pub(super) fn request_call(
    ctx: &mut StepCtx,
    chunk: Arc<Chunk>,
    closure: Option<Arc<ClosureData>>,
    dst: u16,
    abase: u16,
    argc: u16,
    type_env: TypeEnv,
) -> Result<Flow> {
    // A forwarder's arity is a guess. `u8::saturating_add` handed to `fold` takes 2 where the
    // guess was 1.
    let chunk = if chunk.path_forwarder && argc as usize != chunk.num_params {
        path_call_chunk(chunk.paths[0].clone(), argc as usize)
    } else {
        chunk
    };
    if argc as usize != chunk.num_params {
        bail!(
            "`{}` expects {} args but got {}",
            chunk.name,
            chunk.num_params,
            argc
        );
    }
    ctx.call = Some(CallReq {
        chunk,
        closure,
        dst,
        abase: abase as usize,
        argc: argc as usize,
        type_env,
    });
    Ok(Flow::Call)
}

pub(super) fn call_path(
    ctx: &mut StepCtx,
    dst: u16,
    path: u16,
    abase: u16,
    argc: u16,
) -> Result<Flow> {
    let (vm, cur) = (ctx.vm, ctx.cur);
    let (abase, argc) = (abase as usize, argc as usize);
    let path = &cur.paths[path as usize];
    // `::unreachable_match` and friends
    match path.id {
        PathId::UnreachableMatch => bail!("no match arm matched the value"),
        PathId::AssertFailed => bail!("assertion failed"),
        PathId::EnsureFail => {
            let message = if argc > 0 {
                ctx.stack[ctx.base + abase].display()
            } else {
                "condition failed".to_string()
            };
            return Ok(ctx.set(dst, Value::err(Value::str(message))));
        }
        _ => {}
    }
    let call_args = ctx.take_range(abase, argc);
    // typed json parses straight into the target structs
    if let Some(ty) = &path.coerce
        && path.id == PathId::SerdeJsonFromStr
    {
        return Ok(ctx.set(dst, vm.typed_from_str(&call_args, ty, ctx.cur_tenv)?));
    }
    let mut v = vm.dispatch_call(path, call_args)?;
    if let Some(ty) = &path.coerce {
        v = vm.coerce_result(v, ty);
    }
    Ok(ctx.set(dst, v))
}

pub(super) fn path_value(ctx: &mut StepCtx, dst: u16, path: u16) -> Result<Flow> {
    let path = &ctx.cur.paths[path as usize];
    Ok(ctx.set(dst, ctx.vm.eval_path_value(path)?))
}

pub(super) fn make_vec(ctx: &mut StepCtx, dst: u16, first: u16, count: u16) -> Flow {
    let items = ctx.take_range(first as usize, count as usize);
    ctx.set(dst, Value::vec(items))
}

pub(super) fn make_tuple(ctx: &mut StepCtx, dst: u16, first: u16, count: u16) -> Flow {
    let items = ctx.take_range(first as usize, count as usize);
    ctx.set(dst, Value::tuple(items))
}

pub(super) fn array_repeat(ctx: &mut StepCtx, dst: u16, val: u16, count: u16) -> Result<Flow> {
    let n = match ctx.get(count) {
        Value::Int(n) => usize::try_from(*n)?,
        v if v.untag_int().is_some() => usize::try_from(v.untag_int().unwrap())?,
        _ => bail!("array repeat length must be an integer"),
    };
    let v = ctx.get(val).clone();
    Ok(ctx.set(dst, Value::vec(repeat_n(v, n).collect())))
}

pub(super) fn make_range(
    ctx: &mut StepCtx,
    dst: u16,
    start: u16,
    end: u16,
    inclusive: bool,
) -> Result<Flow> {
    let start = int_of(ctx.get(start))?;
    let end = int_of(ctx.get(end))?;
    Ok(ctx.set(
        dst,
        Value::Range {
            start,
            end,
            inclusive,
        },
    ))
}

pub(super) fn for_next(ctx: &mut StepCtx, iter: u16, idx: u16, val: u16, to: u32) -> Result<Flow> {
    let i = match ctx.get(idx) {
        Value::Int(i) => *i,
        _ => unreachable!("for index is an integer"),
    };
    // simple sources produce their item in place and skip `iterator_next`
    let item = {
        let Value::Native(iterator) = ctx.get(iter) else {
            bail!("{} is not an iterator", ctx.get(iter).type_name());
        };
        let fast = match &mut *iterator.lock() {
            Native::Iterator(state) => state.fast_next(),
            _ => FastNext::NotSimple,
        };
        match fast {
            FastNext::Ready(item) => item,
            FastNext::NotSimple => {
                let iterator = iterator.clone();
                ctx.vm.iterator_next(&iterator)?
            }
        }
    };
    let Some(v) = item else {
        return Ok(Flow::Jump(to as usize));
    };
    ctx.put(val, v);
    ctx.vm.run_pending_ctrlc()?;
    Ok(ctx.set(idx, Value::Int(i + 1)))
}

pub(super) fn make_struct(ctx: &mut StepCtx, dst: u16, info: u16, first: u16) -> Flow {
    let lit = &ctx.cur.struct_lits[info as usize];
    let written = lit.shape.fields.len();
    let mut values = ctx.take_range(first as usize, written);
    let v = if lit.has_rest {
        let rest = ctx.stack[ctx.base + first as usize + written].clone();
        let mut fields = lit.shape.fields.clone();
        let mut renames = lit.shape.renames.clone();
        if let Value::Struct(r) = rest {
            let rvals = r.values.lock();
            for (slot, (k, v)) in r.shape.fields.iter().zip(rvals.iter()).enumerate() {
                match lit.shape.slot(k) {
                    Some(index) if !lit.filled.get(index).copied().unwrap_or(true) => {
                        values[index] = v.clone();
                    }
                    Some(_) => {}
                    // the struct definition was out of reach at compile time
                    None => {
                        fields.push(k.clone());
                        values.push(v.clone());
                        if !renames.is_empty() {
                            renames.push(r.shape.renames.get(slot).cloned().flatten());
                        }
                    }
                }
            }
        }
        let shape = StructShape::typed(lit.shape.name.clone(), lit.shape.type_id, fields, renames);
        Value::structure(shape, values)
    } else {
        Value::structure(lit.shape.clone(), values)
    };
    ctx.set(dst, v)
}

pub(super) fn make_enum(ctx: &mut StepCtx, dst: u16, info: u16, first: u16, count: u16) -> Flow {
    let variant = &ctx.cur.enum_variants[info as usize];
    let data = Arc::new(Mutex::new(ctx.take_range(first as usize, count as usize)));
    ctx.set(
        dst,
        Value::Enum {
            def: variant.def.clone(),
            variant: variant.variant,
            data,
        },
    )
}

pub(super) fn load_enum(ctx: &mut StepCtx, dst: u16, info: u16) -> Flow {
    let variant = &ctx.cur.enum_variants[info as usize];
    ctx.set(
        dst,
        Value::Enum {
            def: variant.def.clone(),
            variant: variant.variant,
            data: Arc::new(Mutex::new(Vec::new())),
        },
    )
}

pub(super) fn closure_op(ctx: &mut StepCtx, dst: u16, child: u16) -> Flow {
    let clo = make_closure(ctx, child);
    ctx.set(dst, Value::Closure(clo))
}

pub(super) fn spawn_op(ctx: &mut StepCtx, dst: u16, child: u16) -> Flow {
    let clo = make_closure(ctx, child);
    let interp = ctx.vm.clone();
    let handle = ctx.vm.rt.spawn_blocking(move || {
        match interp.run_chunk(&clo.chunk, &[], &clo.captured) {
            Ok(v) => v,
            // A task panic prints and the join handle gives `Err(JoinError)`, like real tokio.
            // `resume_unwind` skips the hook so the header is not printed twice.
            Err(e) => {
                if let Some(p) = e.downcast_ref::<crate::interpreter::vm_support::ScriptPanic>() {
                    eprint!("{}", p.header("tokio-rt-worker"));
                } else {
                    eprintln!("rust error in task: {e:#}");
                }
                // the bare message, so the `JoinError` formats like tokio's `task 11 panicked
                // with message "boom"`
                let payload = match e.downcast_ref::<crate::interpreter::vm_support::ScriptPanic>()
                {
                    Some(p) => p.message.clone(),
                    None => format!("{e:#}"),
                };
                std::panic::resume_unwind(Box::new(payload))
            }
        }
    });
    ctx.set(dst, Native::Task(handle).wrap())
}

pub(super) fn make_closure(ctx: &mut StepCtx, child: u16) -> Arc<ClosureData> {
    let cur = ctx.cur;
    let child_chunk = cur.children[child as usize].clone();
    let caps = &cur.child_caps[child as usize];
    let takes = &cur.child_moves[child as usize];
    // A `move` closure owns its captures. A local that is dead after this op moves in, a live
    // one is `Copy` and copies. A plain closure shares.
    let moves = child_chunk.moves;
    let own = |value: Value| Upvalue::Mutable(Arc::new(Mutex::new(value)));
    let captured: Vec<Upvalue> = caps
        .iter()
        .zip(takes.iter())
        .map(|(c, &taken)| match c {
            CapSource::Local(reg) => {
                let slot = ctx.base + *reg as usize;
                Upvalue::Value(if taken {
                    take(&mut ctx.stack[slot])
                } else if moves {
                    ctx.stack[slot].deep_clone()
                } else {
                    ctx.stack[slot].clone()
                })
            }
            CapSource::Upvalue(idx) => ctx.upvalues()[*idx as usize].clone(),
            CapSource::MutableUpvalue(idx) => {
                let shared = ctx.upvalues()[*idx as usize].clone();
                if moves {
                    own(shared.get().deep_clone())
                } else {
                    shared
                }
            }
            CapSource::MutableLocal(reg) => {
                let cell = ctx.cell(*reg).clone();
                if taken {
                    own(take(&mut *cell.lock()))
                } else if moves {
                    let value = cell.lock().deep_clone();
                    own(value)
                } else {
                    Upvalue::Mutable(cell)
                }
            }
        })
        .collect();
    Arc::new(ClosureData {
        chunk: child_chunk,
        captured,
    })
}
