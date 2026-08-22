//! The while and loop side of the scalar plans. The plan IR lives in `scalar_loop`. This side also
//! runs vec indexing, with the bases locked for the run, writes journaled and the registers
//! snapshot at every iteration boundary, so a failing iteration restores its entry state.

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

/// Backward jumps between Ctrl-C polls. The vec runner drops its locks around each poll.
const WHILE_POLL: u32 = 65_536;

/// bounds the replay a failure needs
const WHILE_SNAPSHOT: u32 = 4096;

/// zero progress failures before a plan stops being retried
const MAX_ZERO_FAILS: u32 = 32;

/// bounds the entry split and lock cost
const MAX_VECS: usize = 8;

/// Every field access site holds its own element temporary, so a few structs still need a dozen handles.
const MAX_HANDLES: usize = 32;

/// The whole region from the head to the backward `Jump`. Its only exits are the jump back and the
/// jumps to the op after it, so the plan and the generic path leave the loop at the same point.
pub struct WhilePlan {
    ops: Vec<LOp>,
    regs: Vec<u16>,
    /// plus whether the region writes it, which decides the entry split
    vecs: Vec<u16>,
    written: Vec<bool>,
    /// see `handle_regs`
    num_handles: usize,
    /// past `MAX_ZERO_FAILS` the plan is dropped
    fails: AtomicU32,
}

/// The vec bases in first appearance order, plus which ones the region writes. A `UniqueReg` target
/// counts as written, the compiler only emits it before a mutation.
fn vec_bases(chunk: &Chunk, head: usize, exit: usize) -> Option<(Vec<u16>, Vec<bool>)> {
    let mut vecs: Vec<u16> = Vec::new();
    let mut written: Vec<bool> = Vec::new();
    for op in &chunk.code[head..exit] {
        let (base, writes) = match op {
            Op::Index { base, .. } => (*base, false),
            // a `UniqueIndex` splits the element, so its base counts as written
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

/// Each `dst` of an index into a base whose value the region reads fields through. Any other
/// field access rejects the plan in `translate_vec`.
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

/// Every handle use must be preceded by its `ElemRef` on every path from the iteration start, a jump
/// target invalidates every handle. On failure the generic re-run has pre loop values in the
/// handle registers.
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

/// None when any op falls outside the subset. A jump leaving the region anywhere but the shared
/// exit rejects the plan through `target`.
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
    // a register can't serve 2 tables at once, a region that mixes roles stays generic
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

/// Re-run the iterations that finished since the snapshot. A scalar only body touches nothing but
/// registers, so the replay is deterministic. Vec plans snapshot every iteration instead.
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

fn note_fail(plan: &WhilePlan, rejected: &AtomicU8, advanced: bool) {
    if !advanced && plan.fails.fetch_add(1, Ordering::Relaxed) + 1 >= MAX_ZERO_FAILS {
        rejected.store(1, Ordering::Relaxed);
    }
}

enum WhileOut {
    Exit,
    Fail,
}

/// `None` means the generic path should take the jump itself, with the frame rebuilt to the start
/// of the unfinished iteration.
pub(super) fn try_run_while(ctx: &mut StepCtx, head: usize) -> Result<Option<Flow>> {
    let jump_ip = ctx.ip;
    try_run(ctx, head, jump_ip)
}

/// The `LoopHead` entry, so the plan takes over before the first iteration. `None` falls through
/// into the head.
pub(super) fn try_run_entry(ctx: &mut StepCtx, jump_ip: usize) -> Result<Option<Flow>> {
    let head = ctx.ip + 1;
    try_run(ctx, head, jump_ip)
}

fn try_run(ctx: &mut StepCtx, head: usize, jump_ip: usize) -> Result<Option<Flow>> {
    // a rejected loop costs 1 atomic load, never the mutex of the plan map
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

/// Registers only. Failure recovery replays from a periodic snapshot, deterministic because there
/// are no side effects.
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
        // Poll Ctrl-C in place, with the registers written back first so an interrupt unwinds
        // over a consistent frame.
        if work >= WHILE_POLL {
            write_regs(ctx, &plan.regs, &regs);
            ctx.vm.run_pending_ctrlc()?;
            work = 0;
        }
        // the last op is the backward jump, the lookup only guards a plan bug
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
                // only backward jumps count, so straight runs pay no counter
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

/// Undone newest first so a doubly written location ends on its original value.
enum Undo {
    Elem {
        vec: u16,
        idx: usize,
        old: Value,
    },
    /// the arc keeps the write undoable after an `ElemBack` moved the element
    Field {
        data: Arc<StructData>,
        slot: usize,
        old: Value,
    },
}

/// Kept across lock drops for Ctrl-C polls.
struct VecRun {
    regs: Vec<SVal>,
    snapshot: Vec<SVal>,
    handles: Vec<Option<Arc<StructData>>>,
    undo: Vec<Undo>,
    /// Elements already split this run. The handle table and journal hold internal references, so
    /// splitting per `UniqueIndex` would clone on every write. 1 split per element per run gives
    /// the same observable state.
    split: Vec<Vec<bool>>,
    /// Per op member slot cache. Field names resolve by string, the loop would pay that millions
    /// of times for the same sites.
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

/// Split each written base and take the storage handles. `None` when a base is not a vec or 2
/// bases share 1 storage.
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

/// The locks drop around every Ctrl-C poll. A written base is unique to this frame, so nothing
/// can touch it in between.
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

/// Run until the loop exits, an iteration fails or the locks should drop for a poll. Every
/// iteration boundary snapshots and clears the journal.
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

/// Mirrors `Vm::get_field` and `Vm::set_field`.
fn member_slot(data: &StructData, member: &Member) -> Option<usize> {
    match member {
        Member::Named(name) => name.slot_in(&data.shape),
        Member::Indexed(i) => Some(*i),
    }
}

/// Each op remembers the slot it found for a shape, see `VecRun::slots`.
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

/// A non scalar element fails over instead of loading as `Opaque`, that would skip writeback and
/// leave the register stale.
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

/// journaled
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

/// The element arc into the handle table, split once per element per run for a `UniqueIndex`, see
/// `VecRun::split`. A pure split needs no undo entry. False fails over.
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

/// A non scalar field fails over, same writeback reason as `VecGet`.
#[inline]
fn field_get(run: &mut VecRun, ip: usize, dst: u16, handle: u16, member: &Member) -> bool {
    let Some(data) = &run.handles[usize::from(handle)] else {
        return false;
    };
    // the slot cache inline so the read path pays no arc clone
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

/// journaled
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

/// The `SetIndex` writeback of a place chain, journaled.
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

/// Registers from the snapshot, writes by unwinding the journal newest first.
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
