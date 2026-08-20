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

use super::bytecode::{Chunk, Member, Op};
use super::scalar_fold::fold_moves;
use super::scalar_loop::{
    LOp, LTo, NO_SLOT, OpOut, PlanVecs, Region, eval_op, translate, write_regs,
};
use super::scalar_reads::chunk_reads;
use super::scalar_val::{SVal, s_index, s_value};
use super::value::{List, StructData, Value};
use super::vm::Vm;
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

/// Handle table cap per plan, bounding the run state. Every field access
/// site holds its own element temporary, so a body touching a few structs
/// still needs a dozen or more handles.
const MAX_HANDLES: usize = 32;

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
    /// Entries in the run's struct element handle table, see `handle_regs`.
    num_handles: usize,
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
            // A `UniqueIndex` splits the element it loads, so its base
            // counts as written even when nothing stores back.
            Op::SetIndex { base, .. }
            | Op::UniqueIndex { base, .. }
            | Op::UniqueReg { reg: base } => (*base, true),
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

/// The registers that hold a struct element pulled out of a vec base: each
/// `dst` of an index into a base whose value the region then reads or
/// writes fields through. A field access through any other register rejects
/// the plan in `translate_vec`.
fn handle_regs(chunk: &Chunk, head: usize, exit: usize, bases: &[u16]) -> Option<Vec<u16>> {
    let mut field_bases: Vec<u16> = Vec::new();
    for op in &chunk.code[head..exit] {
        let (Op::GetField { base, .. } | Op::UniqueField { base, .. } | Op::SetField { base, .. }) =
            op
        else {
            continue;
        };
        if !field_bases.contains(base) {
            field_bases.push(*base);
        }
    }
    let mut handles: Vec<u16> = Vec::new();
    for op in &chunk.code[head..exit] {
        let (Op::Index { dst, base, .. } | Op::UniqueIndex { dst, base, .. }) = op else {
            continue;
        };
        if bases.contains(base) && field_bases.contains(dst) && !handles.contains(dst) {
            if handles.len() >= MAX_HANDLES {
                return None;
            }
            handles.push(*dst);
        }
    }
    Some(handles)
}

/// Whether every field-op use of a handle is preceded, on every path from
/// an iteration's start, by an `ElemRef` writing it: walking the ops in
/// order, a jump-target position invalidates every handle, since control
/// can land there without executing the def above it. This matters on
/// failure, where the generic path re-runs the iteration from its start
/// with the frame's handle registers holding pre-loop values, so a use the
/// plan could reach without a def in the same iteration would read a value
/// the live generic run would not.
fn handles_dominated(ops: &[LOp], num_handles: usize) -> bool {
    let mut targets = vec![false; ops.len() + 1];
    for op in ops {
        let (LOp::Jump { to }
        | LOp::JumpIfFalse { to, .. }
        | LOp::JumpIfTrue { to, .. }
        | LOp::CmpJump { to, .. }
        | LOp::CmpJumpImm { to, .. }) = op
        else {
            continue;
        };
        if let LTo::Op(t) = to {
            targets[*t as usize] = true;
        }
    }
    let mut defined = vec![false; num_handles];
    for (i, op) in ops.iter().enumerate() {
        if targets[i] {
            defined.fill(false);
        }
        match op {
            LOp::ElemRef { handle, .. } => defined[usize::from(*handle)] = true,
            LOp::FieldGet { handle, .. }
            | LOp::FieldSet { handle, .. }
            | LOp::ElemBack { handle, .. }
                if !defined[usize::from(*handle)] =>
            {
                return false;
            }
            _ => {}
        }
    }
    true
}

/// Translate the loop the backward `Jump` at `jump_ip` closes, or answer
/// None when any op falls outside the subset. A jump leaving the region
/// anywhere but the shared exit, a labeled break out of an outer loop for
/// one, rejects the plan here through `target`.
fn build_while(vm: &Vm, chunk: &Chunk, head: usize, jump_ip: usize) -> Option<WhilePlan> {
    let exit = jump_ip + 1;
    let (vecs, written) = vec_bases(chunk, head, exit)?;
    let handles = handle_regs(chunk, head, exit, &vecs)?;
    let mut regs: Vec<u16> = Vec::new();
    let mut try_mask = 0u64;
    let mut ops = chunk.code[head..exit]
        .iter()
        .map(|op| {
            translate(
                vm,
                chunk,
                &Region {
                    head,
                    body: head,
                    exit,
                },
                &mut regs,
                Some(&PlanVecs {
                    bases: &vecs,
                    handles: &handles,
                }),
                &mut try_mask,
                op,
            )
        })
        .collect::<Option<Vec<_>>>()?;
    // A register cannot serve two tables at once: a locked vec, a scalar
    // slot, and a handle are disjoint roles, and a region that mixes them,
    // moving a base or a handle around for one, stays generic.
    if regs.iter().any(|reg| vecs.contains(reg)) {
        return None;
    }
    if handles.iter().any(|h| vecs.contains(h) || regs.contains(h)) {
        return None;
    }
    fold_moves(&mut ops, NO_SLOT, &chunk_reads(chunk), &regs);
    if !handles_dominated(&ops, handles.len()) {
        return None;
    }
    Some(WhilePlan {
        ops,
        regs,
        vecs,
        written,
        num_handles: handles.len(),
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
    try_run(ctx, head, jump_ip)
}

/// The `LoopHead` entry into the same machinery: `ctx.ip` sits on the
/// `LoopHead` op right before the head, and the operand carries the backward
/// jump, so the plan takes over before the first iteration runs. `None`
/// falls through into the head generically.
pub(super) fn try_run_entry(ctx: &mut StepCtx, jump_ip: usize) -> Result<Option<Flow>> {
    let head = ctx.ip + 1;
    try_run(ctx, head, jump_ip)
}

fn try_run(ctx: &mut StepCtx, head: usize, jump_ip: usize) -> Result<Option<Flow>> {
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
            let built = build_while(ctx.vm, ctx.cur, head, jump_ip).map(Arc::new);
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

/// One journaled write of the current iteration, undone newest first on
/// failure so a doubly-written location ends on its original value.
enum Undo {
    /// A vec element replaced through `VecSet` or `ElemBack`.
    Elem { vec: u16, idx: usize, old: Value },
    /// A struct field replaced through `FieldSet`. The arc keeps the write
    /// undoable even after an `ElemBack` moved the element around.
    Field {
        data: Arc<StructData>,
        slot: usize,
        old: Value,
    },
}

/// The live state of one vec plan run, kept across lock drops for Ctrl-C
/// polls: the registers, their copy from the current iteration's entry, the
/// struct element handles, and the journal of this iteration's writes.
struct VecRun {
    regs: Vec<SVal>,
    snapshot: Vec<SVal>,
    handles: Vec<Option<Arc<StructData>>>,
    undo: Vec<Undo>,
    /// Elements already split this run, one lazy bitmap per vec. The handle
    /// table and the journal hold plan-internal references the script never
    /// sees, so splitting per generic `UniqueIndex` would clone the struct
    /// on every write access. One split per element per run is the same
    /// observable state.
    split: Vec<Vec<bool>>,
    /// Per-op member slot cache: the shape address the op last resolved
    /// against, and the values slot it found. Field names resolve by string
    /// against a shape, which is per-access cost the loop pays millions of
    /// times for the same handful of sites.
    slots: Vec<(usize, u32)>,
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
        handles: vec![None; plan.num_handles],
        undo: Vec::new(),
        split: vec![Vec::new(); plan.vecs.len()],
        slots: vec![(0, 0); plan.ops.len()],
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
                if !vec_get(run, guards, *dst, *vec, *idx) {
                    return fail_vec(run, guards);
                }
                run.ip += 1;
            }
            LOp::VecSet { vec, idx, val } => {
                if !vec_set(run, guards, *vec, *idx, *val) {
                    return fail_vec(run, guards);
                }
                run.ip += 1;
            }
            LOp::ElemRef {
                handle,
                vec,
                idx,
                unique,
            } => {
                if !elem_ref(run, guards, *handle, *vec, *idx, *unique) {
                    return fail_vec(run, guards);
                }
                run.ip += 1;
            }
            LOp::FieldGet {
                dst,
                handle,
                member,
            } => {
                let ip = run.ip;
                if !field_get(run, ip, *dst, *handle, member) {
                    return fail_vec(run, guards);
                }
                run.ip += 1;
            }
            LOp::FieldSet {
                handle,
                member,
                val,
            } => {
                let ip = run.ip;
                if !field_set(run, ip, *handle, member, *val) {
                    return fail_vec(run, guards);
                }
                run.ip += 1;
            }
            LOp::ElemBack { vec, idx, handle } => {
                if !elem_back(run, guards, *vec, *idx, *handle) {
                    return fail_vec(run, guards);
                }
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

/// The values slot a member names inside a struct, mirroring the member
/// resolution of the generic `Vm::get_field` and `Vm::set_field`. An
/// out-of-range indexed member is caught by the values access itself.
fn member_slot(data: &StructData, member: &Member) -> Option<usize> {
    match member {
        Member::Named(name) => name.slot_in(&data.shape),
        Member::Indexed(i) => Some(*i),
    }
}

/// The values slot a member names against the op's cache: the handle table
/// and the journal make field names resolve by string against a shape,
/// which is per-access cost the loop pays millions of times for the same
/// handful of sites, so each op remembers the slot it found for a shape.
fn cached_slot(
    run: &mut VecRun,
    ip: usize,
    data: &Arc<StructData>,
    member: &Member,
) -> Option<usize> {
    let shape_addr = Arc::as_ptr(&data.shape).addr();
    let (addr, slot) = run.slots[ip];
    if addr == shape_addr {
        return Some(slot as usize);
    }
    let found = member_slot(data, member)?;
    run.slots[ip] = (shape_addr, u32::try_from(found).ok()?);
    Some(found)
}

/// The `VecGet` arm of `run_vec_span`. A non-scalar element fails over
/// instead of loading as `Opaque`: a slot the loop never reads again would
/// skip writeback and leave the frame register stale.
#[inline]
fn vec_get(
    run: &mut VecRun,
    guards: &mut [MutexGuard<'_, Vec<Value>>],
    dst: u16,
    vec: u16,
    idx: u16,
) -> bool {
    let elem = s_index(run.regs[usize::from(idx)])
        .and_then(|i| guards[usize::from(vec)].get(i))
        .map(SVal::of);
    let Some(v @ (SVal::Unit | SVal::Int(_) | SVal::IntW(..) | SVal::Float(_) | SVal::Bool(_))) =
        elem
    else {
        return false;
    };
    run.regs[usize::from(dst)] = v;
    true
}

/// The `VecSet` arm of `run_vec_span`, journaled so a failing iteration
/// can undo it.
#[inline]
fn vec_set(
    run: &mut VecRun,
    guards: &mut [MutexGuard<'_, Vec<Value>>],
    vec: u16,
    idx: u16,
    val: u16,
) -> bool {
    let target = s_index(run.regs[usize::from(idx)]);
    let new = s_value(run.regs[usize::from(val)]);
    let (Some(i), Some(new)) = (target, new) else {
        return false;
    };
    let Some(slot) = guards[usize::from(vec)].get_mut(i) else {
        return false;
    };
    run.undo.push(Undo::Elem {
        vec,
        idx: i,
        old: replace(slot, new),
    });
    true
}

/// The `ElemRef` arm of `run_vec_span`: the element arc of `vec[idx]` into
/// the handle table, split from sharing first when the generic op was a
/// `UniqueIndex`, so field writes cannot leak into a sibling copy. The
/// split runs once per element per run, see `VecRun::split`: after it, the
/// only extra references are the plan's own handle and journal entries,
/// which the generic per-access split would otherwise count as sharing and
/// clone the struct on every write. A pure split needs no undo entry.
/// False fails the iteration over.
#[inline]
fn elem_ref(
    run: &mut VecRun,
    guards: &mut [MutexGuard<'_, Vec<Value>>],
    handle: u16,
    vec: u16,
    idx: u16,
    unique: bool,
) -> bool {
    let Some(i) = s_index(run.regs[usize::from(idx)]) else {
        return false;
    };
    let Some(slot) = guards[usize::from(vec)].get_mut(i) else {
        return false;
    };
    if unique {
        let flags = &mut run.split[usize::from(vec)];
        if flags.len() <= i {
            flags.resize(i + 1, false);
        }
        if !flags[i] {
            slot.make_unique();
            flags[i] = true;
        }
    }
    let Value::Struct(data) = &*slot else {
        return false;
    };
    run.handles[usize::from(handle)] = Some(data.clone());
    true
}

/// The `FieldGet` arm of `run_vec_span`. A non-scalar field fails over
/// instead of loading as `Opaque`, for the same writeback reason as
/// `VecGet`.
#[inline]
fn field_get(run: &mut VecRun, ip: usize, dst: u16, handle: u16, member: &Member) -> bool {
    let Some(data) = &run.handles[usize::from(handle)] else {
        return false;
    };
    // The slot cache inline, on split field borrows, so the read path pays
    // no arc clone, see `cached_slot`.
    let shape_addr = Arc::as_ptr(&data.shape).addr();
    let (addr, cached) = run.slots[ip];
    let slot = if addr == shape_addr {
        cached as usize
    } else {
        let Some(found) = member_slot(data, member) else {
            return false;
        };
        let Ok(compact) = u32::try_from(found) else {
            return false;
        };
        run.slots[ip] = (shape_addr, compact);
        found
    };
    let field = data.values.lock().get(slot).map(SVal::of);
    let Some(v @ (SVal::Unit | SVal::Int(_) | SVal::IntW(..) | SVal::Float(_) | SVal::Bool(_))) =
        field
    else {
        return false;
    };
    run.regs[usize::from(dst)] = v;
    true
}

/// The `FieldSet` arm of `run_vec_span`, journaled so a failing iteration
/// can undo it.
#[inline]
fn field_set(run: &mut VecRun, ip: usize, handle: u16, member: &Member, val: u16) -> bool {
    let Some(data) = run.handles[usize::from(handle)].clone() else {
        return false;
    };
    let new = s_value(run.regs[usize::from(val)]);
    let (Some(slot), Some(new)) = (cached_slot(run, ip, &data, member), new) else {
        return false;
    };
    let old = {
        let mut values = data.values.lock();
        let Some(field) = values.get_mut(slot) else {
            return false;
        };
        replace(field, new)
    };
    run.undo.push(Undo::Field { data, slot, old });
    true
}

/// The `ElemBack` arm of `run_vec_span`: the `SetIndex` writeback of a
/// place chain stores the held element back into its vec slot, journaled
/// like a vec write.
#[inline]
fn elem_back(
    run: &mut VecRun,
    guards: &mut [MutexGuard<'_, Vec<Value>>],
    vec: u16,
    idx: u16,
    handle: u16,
) -> bool {
    let Some(i) = s_index(run.regs[usize::from(idx)]) else {
        return false;
    };
    let Some(data) = &run.handles[usize::from(handle)] else {
        return false;
    };
    let Some(slot) = guards[usize::from(vec)].get_mut(i) else {
        return false;
    };
    run.undo.push(Undo::Elem {
        vec,
        idx: i,
        old: replace(slot, Value::Struct(data.clone())),
    });
    true
}

/// Restore the failing iteration's entry state: registers from the
/// snapshot, vec elements and struct fields by unwinding the write journal
/// newest first, so a doubly-written location ends on its original value.
fn fail_vec(run: &mut VecRun, guards: &mut [MutexGuard<'_, Vec<Value>>]) -> SpanOut {
    run.regs.copy_from_slice(&run.snapshot);
    while let Some(entry) = run.undo.pop() {
        match entry {
            Undo::Elem { vec, idx, old } => guards[usize::from(vec)][idx] = old,
            Undo::Field { data, slot, old } => data.values.lock()[slot] = old,
        }
    }
    SpanOut::Fail
}
