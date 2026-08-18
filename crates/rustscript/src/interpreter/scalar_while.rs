//! The while/loop side of the scalar loop specialization: the whole region
//! a backward `Jump` closes, condition included, runs as a plan inside one
//! dispatch. The plan IR, its translation, and the op evaluator live in
//! `scalar_loop`.
//!
//! Unlike the `for` plan this side also runs vec indexing. The region's vec
//! bases are resolved once at entry, each written base split from sharing
//! the way the generic path's `UniqueReg` would, and their storage stays
//! locked while the plan runs. Vec writes land immediately and go into a
//! journal, and the registers snapshot at every iteration boundary, so a
//! failing iteration restores both exactly to its entry state and the
//! generic loop re-runs it with identical semantics, panic line included.

use std::mem::replace;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use anyhow::Result;
use parking_lot::MutexGuard;

use super::bytecode::{Chunk, Op};
use super::scalar_loop::{LOp, LTo, NO_SLOT, OpOut, eval_op, fold_moves, translate, write_regs};
use super::scalar_reads::chunk_reads;
use super::scalar_val::{SVal, s_index, s_value};
use super::value::{List, Value};
use super::vm_step::{Flow, StepCtx};

/// Backward jumps between Ctrl-C polls. The plan holds no iterator lock, so
/// it polls mid-run instead of failing the iteration over to the generic
/// path, and the vec runner drops its storage locks around each poll.
const WHILE_POLL: u32 = 65_536;

/// Completed iterations between register snapshots in a scalar-only plan,
/// bounding the replay a failure needs.
const WHILE_SNAPSHOT: u32 = 4096;

/// Zero-progress failures before a plan stops being retried. A loop whose
/// entry state never reads as scalars would otherwise pay the load and
/// writeback on every backward jump.
const MAX_ZERO_FAILS: u32 = 32;

/// Vec table cap per plan, bounding the entry split and lock cost.
const MAX_VECS: usize = 8;

/// A plan for a loop closed by a backward `Jump`: the whole region from the
/// loop head to the jump, condition included. The only ways out of such a
/// region are the jump back to the head, one finished iteration, and the
/// jumps to the op right after it, the loop's exit, so the plan and the
/// generic path leave the loop at the same single point.
pub struct WhilePlan {
    ops: Vec<LOp>,
    /// The frame register behind each plan slot.
    regs: Vec<u16>,
    /// The frame register behind each vec table entry, plus whether the
    /// region writes it, which decides the entry split.
    vecs: Vec<u16>,
    written: Vec<bool>,
    /// Runs that failed before finishing one iteration. Past
    /// `MAX_ZERO_FAILS` the plan is dropped, so a loop whose entry state
    /// never reads as scalars stops paying the attempt per backward jump.
    fails: AtomicU32,
}

/// The region's vec base registers in first-appearance order, the table the
/// plan's vec ops index, plus which of them the region writes. A `UniqueReg`
/// target counts as a written base: the compiler emits it only right before
/// a mutation, and a mutation the plan cannot run as a vec write rejects
/// the plan through its own op anyway.
fn vec_bases(chunk: &Chunk, head: usize, exit: usize) -> Option<(Vec<u16>, Vec<bool>)> {
    let mut vecs: Vec<u16> = Vec::new();
    let mut written: Vec<bool> = Vec::new();
    for op in &chunk.code[head..exit] {
        let (base, writes) = match op {
            Op::Index { base, .. } => (*base, false),
            Op::SetIndex { base, .. } | Op::UniqueReg { reg: base } => (*base, true),
            _ => continue,
        };
        if let Some(i) = vecs.iter().position(|&r| r == base) {
            written[i] = written[i] || writes;
            continue;
        }
        if vecs.len() >= MAX_VECS {
            return None;
        }
        vecs.push(base);
        written.push(writes);
    }
    Some((vecs, written))
}

/// Translate the loop the backward `Jump` at `jump_ip` closes, or answer
/// None when any op falls outside the subset. A jump leaving the region
/// anywhere but the shared exit, a labeled break out of an outer loop for
/// one, rejects the plan here through `target`.
fn build_while(chunk: &Chunk, head: usize, jump_ip: usize) -> Option<WhilePlan> {
    let exit = jump_ip + 1;
    let (vecs, written) = vec_bases(chunk, head, exit)?;
    let mut regs: Vec<u16> = Vec::new();
    let mut ops = chunk.code[head..exit]
        .iter()
        .map(|op| translate(chunk, head, head, exit, &mut regs, Some(&vecs), op))
        .collect::<Option<Vec<_>>>()?;
    // A register cannot be a locked vec and a scalar slot at once; a region
    // that moves a base around or overwrites it stays generic.
    if regs.iter().any(|reg| vecs.contains(reg)) {
        return None;
    }
    fold_moves(&mut ops, NO_SLOT, &chunk_reads(chunk), &regs);
    Some(WhilePlan {
        ops,
        regs,
        vecs,
        written,
        fails: AtomicU32::new(0),
    })
}

/// Rebuild the registers to the start of the failing iteration: the caller
/// restores the snapshot, this re-runs the iterations that finished since it
/// was taken. A scalar-only body touches nothing but registers, so the
/// replay is deterministic and cannot exit or fail where the live run did
/// not. Vec plans never replay, they snapshot every iteration instead.
fn replay_while(plan: &WhilePlan, regs: &mut [SVal], count: u32) {
    let mut done = 0u32;
    let mut ip = 0usize;
    while done < count {
        let Some(op) = plan.ops.get(ip) else {
            unreachable!("replayed iteration diverged");
        };
        match eval_op(op, regs) {
            OpOut::Fall => ip += 1,
            OpOut::Jump(LTo::Next) => {
                done += 1;
                ip = 0;
            }
            OpOut::Jump(LTo::Op(t)) => ip = t as usize,
            OpOut::Fail | OpOut::Jump(LTo::Exit) => unreachable!("replayed iteration diverged"),
        }
    }
}

/// Count one failed run, and past the zero-progress budget reject the loop
/// for good.
fn note_fail(plan: &WhilePlan, rejected: &AtomicU8, advanced: bool) {
    if !advanced && plan.fails.fetch_add(1, Ordering::Relaxed) + 1 >= MAX_ZERO_FAILS {
        rejected.store(1, Ordering::Relaxed);
    }
}

enum WhileOut {
    Exit,
    Fail,
}

/// Try to run the loop the backward `Jump` under `ctx.ip` closes as a scalar
/// plan, starting at the head the jump targets. `None` means the generic
/// path should take the jump itself, with the frame rebuilt to the start of
/// the iteration the plan could not finish, so the generic loop re-runs that
/// iteration with identical semantics.
pub(super) fn try_run_while(ctx: &mut StepCtx, head: usize) -> Result<Option<Flow>> {
    let jump_ip = ctx.ip;
    // The backward jump of a rejected loop runs once per iteration, so its
    // answer is a plain atomic load, never the plan map's mutex.
    let Some(rejected) = ctx.cur.while_rejected.get(jump_ip) else {
        return Ok(None);
    };
    if rejected.load(Ordering::Relaxed) != 0 {
        return Ok(None);
    }
    let plan = {
        let mut plans = ctx.cur.while_plans.lock();
        if let Some(cached) = plans.get(&jump_ip) {
            Some(cached.clone())
        } else {
            let built = build_while(ctx.cur, head, jump_ip).map(Arc::new);
            match &built {
                Some(plan) => {
                    plans.insert(jump_ip, plan.clone());
                }
                None => rejected.store(1, Ordering::Relaxed),
            }
            built
        }
    };
    let Some(plan) = plan else { return Ok(None) };
    if plan.vecs.is_empty() {
        run_scalar(ctx, &plan, jump_ip, rejected)
    } else {
        run_vec(ctx, &plan, jump_ip, rejected)
    }
}

/// Run a plan whose region touches nothing but registers. Failure recovery
/// replays finished iterations from a periodic snapshot, which stays
/// deterministic exactly because there are no side effects to re-read.
fn run_scalar(
    ctx: &mut StepCtx,
    plan: &WhilePlan,
    jump_ip: usize,
    rejected: &AtomicU8,
) -> Result<Option<Flow>> {
    let mut regs: Vec<SVal> = plan.regs.iter().map(|&r| SVal::of(ctx.get(r))).collect();
    let mut snapshot = regs.clone();
    let mut since_snapshot: u32 = 0;
    let mut advanced = false;
    let mut work: u32 = 0;
    let mut ip = 0usize;
    let out = loop {
        // The plan holds no locks, so a long run polls Ctrl-C in place
        // rather than failing over. The handler runs script in its own
        // frame and cannot see this one's registers, but they are written
        // back first so an interrupt error unwinds over a consistent frame.
        if work >= WHILE_POLL {
            write_regs(ctx, &plan.regs, &regs);
            ctx.vm.run_pending_ctrlc()?;
            work = 0;
        }
        // The last op is the loop's own backward jump, so `ip` cannot walk
        // past the end; the lookup only guards a plan bug.
        let Some(op) = plan.ops.get(ip) else {
            break WhileOut::Fail;
        };
        match eval_op(op, &mut regs) {
            OpOut::Fall => ip += 1,
            OpOut::Fail => break WhileOut::Fail,
            OpOut::Jump(LTo::Exit) => break WhileOut::Exit,
            OpOut::Jump(LTo::Next) => {
                advanced = true;
                since_snapshot += 1;
                work += 1;
                if since_snapshot >= WHILE_SNAPSHOT {
                    snapshot.copy_from_slice(&regs);
                    since_snapshot = 0;
                }
                ip = 0;
            }
            OpOut::Jump(LTo::Op(t)) => {
                let t = t as usize;
                // Only backward jumps accrue poll work, the one way a run
                // grows long, so straight runs pay no counter.
                if t <= ip {
                    work += 1;
                }
                ip = t;
            }
        }
    };
    match out {
        WhileOut::Exit => {
            write_regs(ctx, &plan.regs, &regs);
            Ok(Some(Flow::Jump(jump_ip + 1)))
        }
        WhileOut::Fail => {
            regs.copy_from_slice(&snapshot);
            replay_while(plan, &mut regs, since_snapshot);
            write_regs(ctx, &plan.regs, &regs);
            note_fail(plan, rejected, advanced);
            Ok(None)
        }
    }
}

/// The live state of one vec plan run, kept across lock drops for Ctrl-C
/// polls: the registers, their copy from the current iteration's entry, and
/// the journal of this iteration's vec writes.
struct VecRun {
    regs: Vec<SVal>,
    snapshot: Vec<SVal>,
    undo: Vec<(u16, usize, Value)>,
    ip: usize,
    work: u32,
    advanced: bool,
}

enum SpanOut {
    Exit,
    Fail,
    Poll,
}

/// Resolve the plan's vec table against the frame: split each written base
/// from sharing, the one split the generic path's per-write `UniqueReg`
/// amounts to, and take the storage handles. `None` when a base is not a
/// plain vec, or two bases share one storage whose lock the runner cannot
/// take twice; the generic path handles those.
fn vec_setup(ctx: &mut StepCtx, plan: &WhilePlan) -> Option<Vec<List>> {
    let mut handles: Vec<List> = Vec::with_capacity(plan.vecs.len());
    for (&reg, &written) in plan.vecs.iter().zip(&plan.written) {
        if written {
            ctx.stack[ctx.base + usize::from(reg)].make_unique();
        }
        let Value::Vec(list) = ctx.get(reg) else {
            return None;
        };
        handles.push(list.clone());
    }
    let aliased =
        (1..handles.len()).any(|i| handles[..i].iter().any(|h| Arc::ptr_eq(h, &handles[i])));
    (!aliased).then_some(handles)
}

/// Run a plan whose region indexes vecs. The storage locks drop around
/// every Ctrl-C poll, the write journal and register snapshot carry across
/// the gap, and a written base is unique to this frame, so nothing can
/// touch it in between.
fn run_vec(
    ctx: &mut StepCtx,
    plan: &WhilePlan,
    jump_ip: usize,
    rejected: &AtomicU8,
) -> Result<Option<Flow>> {
    let Some(handles) = vec_setup(ctx, plan) else {
        note_fail(plan, rejected, false);
        return Ok(None);
    };
    let regs: Vec<SVal> = plan.regs.iter().map(|&r| SVal::of(ctx.get(r))).collect();
    let mut run = VecRun {
        snapshot: regs.clone(),
        regs,
        undo: Vec::new(),
        ip: 0,
        work: 0,
        advanced: false,
    };
    loop {
        let out = {
            let mut guards: Vec<_> = handles.iter().map(|h| h.lock()).collect();
            run_vec_span(plan, &mut run, &mut guards)
        };
        write_regs(ctx, &plan.regs, &run.regs);
        match out {
            SpanOut::Poll => {
                ctx.vm.run_pending_ctrlc()?;
                run.work = 0;
            }
            SpanOut::Exit => return Ok(Some(Flow::Jump(jump_ip + 1))),
            SpanOut::Fail => {
                note_fail(plan, rejected, run.advanced);
                return Ok(None);
            }
        }
    }
}

/// Run plan ops until the loop exits, an iteration fails, or enough work
/// accrues that the locks should drop for a poll. Every iteration boundary
/// snapshots the registers and clears the journal, so a failure restores
/// the failing iteration's entry state exactly.
fn run_vec_span(
    plan: &WhilePlan,
    run: &mut VecRun,
    guards: &mut [MutexGuard<'_, Vec<Value>>],
) -> SpanOut {
    loop {
        if run.work >= WHILE_POLL {
            return SpanOut::Poll;
        }
        let Some(op) = plan.ops.get(run.ip) else {
            return fail_vec(run, guards);
        };
        match op {
            LOp::VecGet { dst, vec, idx } => {
                let elem = s_index(run.regs[usize::from(*idx)])
                    .and_then(|i| guards[usize::from(*vec)].get(i))
                    .map(SVal::of);
                // A non-scalar element fails over instead of loading as
                // `Opaque`: a slot the loop never reads again would skip
                // writeback and leave the frame register stale.
                let Some(v @ (SVal::Unit | SVal::Int(_) | SVal::IntW(..) | SVal::Bool(_))) = elem
                else {
                    return fail_vec(run, guards);
                };
                run.regs[usize::from(*dst)] = v;
                run.ip += 1;
            }
            LOp::VecSet { vec, idx, val } => {
                let target = s_index(run.regs[usize::from(*idx)]);
                let new = s_value(run.regs[usize::from(*val)]);
                let (Some(i), Some(new)) = (target, new) else {
                    return fail_vec(run, guards);
                };
                let Some(slot) = guards[usize::from(*vec)].get_mut(i) else {
                    return fail_vec(run, guards);
                };
                run.undo.push((*vec, i, replace(slot, new)));
                run.ip += 1;
            }
            other => match eval_op(other, &mut run.regs) {
                OpOut::Fall => run.ip += 1,
                OpOut::Fail => return fail_vec(run, guards),
                OpOut::Jump(LTo::Exit) => return SpanOut::Exit,
                OpOut::Jump(LTo::Next) => {
                    run.advanced = true;
                    run.work += 1;
                    run.undo.clear();
                    run.snapshot.copy_from_slice(&run.regs);
                    run.ip = 0;
                }
                OpOut::Jump(LTo::Op(t)) => {
                    let t = t as usize;
                    if t <= run.ip {
                        run.work += 1;
                    }
                    run.ip = t;
                }
            },
        }
    }
}

/// Restore the failing iteration's entry state: registers from the
/// snapshot, vec elements by unwinding the write journal newest first, so
/// a doubly-written element ends on its original value.
fn fail_vec(run: &mut VecRun, guards: &mut [MutexGuard<'_, Vec<Value>>]) -> SpanOut {
    run.regs.copy_from_slice(&run.snapshot);
    while let Some((vec, i, old)) = run.undo.pop() {
        guards[usize::from(vec)][i] = old;
    }
    SpanOut::Fail
}
