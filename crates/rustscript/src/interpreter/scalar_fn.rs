//! Scalar function plans: a self-recursive function whose whole body
//! compiles to integer, float, and bool bytecode runs unboxed inside one
//! `CallFn` dispatch. The generic path pays a frame push, boxed `Value`
//! registers, and one dispatch per op for every call, which is the whole
//! cost of a call tree like `fib`. The plan runs the tree on a flat stack
//! of unboxed values with no `Value` anywhere.
//!
//! The subset makes such a body pure: no globals, no cells, no upvalues,
//! no vec or field access, and the only calls are recursion into the same
//! plan and the whitelisted numeric methods. So failure recovery needs no
//! journal and no replay. Any runtime surprise, an overflow, an unsupported
//! width, a non-scalar argument, discards the run and the generic path runs
//! the whole call from scratch with identical semantics, panic op and line
//! included.
//!
//! The plan IR, its translation, and the op evaluator live in
//! `scalar_loop`, the runner here holds the frame stack the `CallSelf` and
//! `Ret` ops need.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::Result;

use super::bytecode::{Chunk, Op};
use super::scalar_fold::fold_moves;
use super::scalar_loop::{
    LOp, LTo, MAX_CALL_ARGS, NO_SLOT, OpOut, Region, eval_op, slot, translate,
};
use super::scalar_reads::chunk_reads;
use super::scalar_val::{SVal, s_value};
use super::value::Value;
use super::vm::{MAX_CALL_DEPTH, Vm};
use super::vm_step::StepCtx;

/// Calls and backward jumps between Ctrl-C polls, matching the while plan's
/// cadence. The plan holds no locks, so it polls mid-run.
const FN_POLL: u32 = 65_536;

/// Consecutive failed runs before the function is rejected for good, so a
/// function whose arguments never read as scalars stops paying the attempt
/// per call.
const MAX_FAILS: u32 = 32;

/// A plan for one function body. Slots `0..num_params` are the parameters,
/// in order, and every frame of the run is one `num_slots` window on the
/// flat stack.
pub struct FnPlan {
    ops: Vec<LOp>,
    num_slots: usize,
    num_params: usize,
    /// Consecutive failed runs, cleared by a success, see `MAX_FAILS`.
    fails: AtomicU32,
}

/// Translate the whole body of `chunk`, or answer None when the function
/// does not qualify. Only a function whose recursion targets itself plans:
/// for a leaf the generic frame is cheap enough, and calls into other
/// functions would need a plan registry the self shape does not.
fn build(vm: &Vm, chunk: &Arc<Chunk>) -> Option<FnPlan> {
    if chunk.path_forwarder
        || !chunk.generics.is_empty()
        || chunk.num_params > MAX_CALL_ARGS
        || chunk.code.is_empty()
    {
        return None;
    }
    let mut regs: Vec<u16> = (0..u16::try_from(chunk.num_params).ok()?).collect();
    // No op maps to `head`, a function body has no loop to re-enter, and a
    // jump to `exit` falls off the end, which returns unit like the generic
    // frame loop does.
    let region = Region {
        head: usize::MAX,
        body: 0,
        exit: chunk.code.len(),
    };
    let mut recursive = false;
    let mut try_mask = 0u64;
    let mut ops = Vec::with_capacity(chunk.code.len());
    for op in &chunk.code {
        let lop = match op {
            Op::CallFn {
                dst,
                func,
                base,
                argc,
                targ,
            } => {
                let callee = vm.functions.get(*func as usize)?;
                if *targ != u32::MAX
                    || !Arc::ptr_eq(callee, chunk)
                    || usize::from(*argc) != chunk.num_params
                {
                    return None;
                }
                let mut args = [0u16; MAX_CALL_ARGS];
                for (arg, reg) in args.iter_mut().zip(*base..base.saturating_add(*argc)) {
                    *arg = slot(&mut regs, reg)?;
                }
                recursive = true;
                LOp::CallSelf {
                    dst: slot(&mut regs, *dst)?,
                    args,
                    argc: u8::try_from(*argc).ok()?,
                }
            }
            Op::Ret { src } => LOp::Ret {
                src: slot(&mut regs, *src)?,
            },
            other => translate(vm, chunk, &region, &mut regs, None, &mut try_mask, other)?,
        };
        ops.push(lop);
    }
    if !recursive {
        return None;
    }
    fold_moves(&mut ops, NO_SLOT, &chunk_reads(chunk), &regs);
    Some(FnPlan {
        ops,
        num_slots: regs.len(),
        num_params: chunk.num_params,
        fails: AtomicU32::new(0),
    })
}

/// One suspended caller: where to resume and which slot takes the return
/// value. Every frame runs the same plan, so the frame carries no chunk.
struct Frame {
    base: usize,
    ret_ip: usize,
    dst: u16,
}

/// Run one call tree, `None` when an op fails and the generic path should
/// run the whole call instead. `depth_budget` is the frame count left under
/// the generic `MAX_CALL_DEPTH`, so the plan fails over exactly where the
/// generic path would report call depth exceeded.
fn run(vm: &Arc<Vm>, plan: &FnPlan, args: &[SVal], depth_budget: usize) -> Result<Option<SVal>> {
    let slots = plan.num_slots;
    let mut stack: Vec<SVal> = vec![SVal::Unit; slots];
    stack[..args.len()].copy_from_slice(args);
    let mut frames: Vec<Frame> = Vec::new();
    let mut base = 0usize;
    let mut ip = 0usize;
    let mut work = 0u32;
    loop {
        if work >= FN_POLL {
            vm.run_pending_ctrlc()?;
            work = 0;
        }
        // Falling off the end returns unit, like the generic frame loop.
        let returned = match plan.ops.get(ip) {
            None => Some(SVal::Unit),
            Some(LOp::Ret { src }) => Some(stack[base + usize::from(*src)]),
            Some(LOp::CallSelf { dst, args, argc }) => {
                if frames.len() >= depth_budget {
                    return Ok(None);
                }
                let callee = stack.len();
                stack.resize(callee + slots, SVal::Unit);
                for (i, arg) in args[..usize::from(*argc)].iter().enumerate() {
                    stack[callee + i] = stack[base + usize::from(*arg)];
                }
                frames.push(Frame {
                    base,
                    ret_ip: ip + 1,
                    dst: *dst,
                });
                base = callee;
                ip = 0;
                work += 1;
                None
            }
            Some(other) => match eval_op(other, &mut stack[base..base + slots]) {
                OpOut::Fall => {
                    ip += 1;
                    None
                }
                OpOut::Fail | OpOut::Jump(LTo::Next) => return Ok(None),
                OpOut::Jump(LTo::Exit) => Some(SVal::Unit),
                OpOut::Jump(LTo::Op(t)) => {
                    let t = t as usize;
                    // Only backward jumps accrue poll work, the one way an
                    // iteration inside a body runs long; calls are counted
                    // at the push above.
                    if t <= ip {
                        work += 1;
                    }
                    ip = t;
                    None
                }
            },
        };
        if let Some(v) = returned {
            let Some(frame) = frames.pop() else {
                return Ok(Some(v));
            };
            stack.truncate(base);
            base = frame.base;
            ip = frame.ret_ip;
            stack[base + usize::from(frame.dst)] = v;
        }
    }
}

/// Count one failed run, and past the budget reject the function for good.
fn note_fail(plan: &FnPlan, chunk: &Chunk) {
    if plan.fails.fetch_add(1, Ordering::Relaxed) + 1 >= MAX_FAILS {
        chunk.fn_rejected.store(1, Ordering::Relaxed);
    }
}

/// Try to run the direct call of `callee` as a function plan, with the
/// arguments in the caller's arg window at `abase`. `Ok(None)` means the
/// generic path should run the call, with the caller's frame untouched.
pub(super) fn try_call(
    ctx: &StepCtx,
    callee: &Arc<Chunk>,
    abase: u16,
    argc: u16,
) -> Result<Option<Value>> {
    // The check of a rejected function is one atomic load, never the mutex.
    if callee.fn_rejected.load(Ordering::Relaxed) != 0 {
        return Ok(None);
    }
    let plan = {
        let mut cached = callee.fn_plan.lock();
        if let Some(plan) = &*cached {
            plan.clone()
        } else if let Some(plan) = build(ctx.vm, callee).map(Arc::new) {
            *cached = Some(plan.clone());
            plan
        } else {
            callee.fn_rejected.store(1, Ordering::Relaxed);
            return Ok(None);
        }
    };
    if usize::from(argc) != plan.num_params || ctx.depth >= MAX_CALL_DEPTH {
        return Ok(None);
    }
    let mut vals = [SVal::Unit; MAX_CALL_ARGS];
    for (val, reg) in vals.iter_mut().zip(abase..abase.saturating_add(argc)) {
        *val = SVal::of(ctx.get(reg));
        if matches!(*val, SVal::Opaque) {
            note_fail(&plan, callee);
            return Ok(None);
        }
    }
    // The intercepted call itself would be one generic frame, and each plan
    // frame one more, so the budget maps plan depth onto the exact frame
    // count the generic loop caps.
    let budget = MAX_CALL_DEPTH - ctx.depth - 1;
    if let Some(v) = run(ctx.vm, &plan, &vals[..usize::from(argc)], budget)? {
        plan.fails.store(0, Ordering::Relaxed);
        Ok(s_value(v))
    } else {
        note_fail(&plan, callee);
        Ok(None)
    }
}
