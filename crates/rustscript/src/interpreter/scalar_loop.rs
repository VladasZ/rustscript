//! Scalar loop specialization for `for` bodies.
//!
//! A `for` loop over a string's bytes or an integer range pays the full VM
//! machinery per item: the iterator lock, a boxed `Value` per element, and
//! one dispatch per body op. A body that only moves plain integers and
//! booleans does not need any of that. This module translates such a body
//! once per loop into a small plan over unboxed scalar registers and runs
//! the whole loop inside one `ForNext` dispatch.
//!
//! The subset is strict on purpose, and every runtime surprise falls back to
//! the generic path with identical semantics. Register values are loaded at
//! loop entry, a value the plan cannot read is poison that aborts on first
//! read, and arithmetic runs through the same width-checked cores the
//! generic ops use. On any failure the registers are rebuilt to the state at
//! the start of the failing iteration by replaying the current chunk from a
//! snapshot, the source is left with that item unconsumed, and the generic
//! loop re-runs it, so an overflow panics on the exact op and line the
//! generic path panics on.

use std::sync::Arc;

use anyhow::Result;

use super::bytecode::{BinKind, Chunk, Op, UnKind};
use super::iterator::IteratorState;
use super::native::Native;
use super::numeric::IntWidth;
use super::scalar_reads::chunk_reads;
use super::scalar_val::{SVal, s_bin, s_cast, s_cmp, s_un, truthy};
use super::typeir::CastIr;
use super::value::Value;
use super::vm_step::{Flow, StepCtx};

/// Items processed per iterator lock hold. Between chunks the lock drops,
/// the registers write back, and the pending Ctrl-C handler runs, so a long
/// loop cannot starve either.
const CHUNK: usize = 4096;

/// Op budget for one iteration, so an inner loop that runs long falls back
/// to the generic path, which polls Ctrl-C on every backward jump.
const MAX_BODY_STEPS: u32 = 65_536;

/// Register cap per plan, bounding the entry load and writeback cost.
const MAX_SLOTS: usize = 64;

/// A jump target inside the plan. `Next` is the loop head, one finished
/// iteration, and `Exit` is the op after the loop, a `break` or exhaustion.
#[derive(Clone, Copy)]
enum LTo {
    Op(u32),
    Next,
    Exit,
}

/// One plan op. Registers are dense plan slots, not frame registers.
enum LOp {
    LoadUnit {
        dst: u16,
    },
    LoadInt {
        dst: u16,
        v: i64,
    },
    LoadIntW {
        dst: u16,
        v: i64,
        w: IntWidth,
    },
    LoadBool {
        dst: u16,
        v: bool,
    },
    Move {
        dst: u16,
        src: u16,
    },
    Bin {
        dst: u16,
        a: u16,
        b: u16,
        op: BinKind,
    },
    BinImm {
        dst: u16,
        a: u16,
        imm: i64,
        op: BinKind,
    },
    Un {
        dst: u16,
        a: u16,
        op: UnKind,
    },
    Jump {
        to: LTo,
    },
    JumpIfFalse {
        cond: u16,
        to: LTo,
    },
    JumpIfTrue {
        cond: u16,
        to: LTo,
    },
    CmpJump {
        a: u16,
        b: u16,
        op: BinKind,
        to: LTo,
    },
    CmpJumpImm {
        a: u16,
        imm: i64,
        op: BinKind,
        to: LTo,
    },
    Cast {
        dst: u16,
        src: u16,
        w: IntWidth,
    },
}

pub struct LoopPlan {
    ops: Vec<LOp>,
    /// The frame register behind each plan slot.
    regs: Vec<u16>,
    /// The slot of the `ForNext` value register, written per item.
    val_slot: u16,
    /// True when the body is one basic block: no jump ops except the final
    /// `Jump` back to the head. Such a body runs as a plain slice walk with
    /// no instruction pointer.
    straight: bool,
}

fn slot(regs: &mut Vec<u16>, r: u16) -> Option<u16> {
    if let Some(i) = regs.iter().position(|&x| x == r) {
        return u16::try_from(i).ok();
    }
    if regs.len() >= MAX_SLOTS {
        return None;
    }
    regs.push(r);
    u16::try_from(regs.len() - 1).ok()
}

fn target(head: usize, exit: usize, t: u32) -> Option<LTo> {
    let t = t as usize;
    if t == head {
        Some(LTo::Next)
    } else if t == exit {
        Some(LTo::Exit)
    } else if t > head && t < exit {
        u32::try_from(t - head - 1).ok().map(LTo::Op)
    } else {
        None
    }
}

/// Translate one bytecode op, or answer None when it falls outside the
/// subset, which rejects the whole loop.
fn translate(chunk: &Chunk, head: usize, exit: usize, regs: &mut Vec<u16>, op: &Op) -> Option<LOp> {
    Some(match op {
        Op::LoadUnit { dst } => LOp::LoadUnit {
            dst: slot(regs, *dst)?,
        },
        Op::LoadInt { dst, v } => LOp::LoadInt {
            dst: slot(regs, *dst)?,
            v: *v,
        },
        Op::LoadIntW { dst, v, w } if !w.is_big() => LOp::LoadIntW {
            dst: slot(regs, *dst)?,
            v: *v,
            w: *w,
        },
        Op::LoadBool { dst, v } => LOp::LoadBool {
            dst: slot(regs, *dst)?,
            v: *v,
        },
        Op::Move { dst, src } => LOp::Move {
            dst: slot(regs, *dst)?,
            src: slot(regs, *src)?,
        },
        Op::Bin { dst, a, b, op } => LOp::Bin {
            dst: slot(regs, *dst)?,
            a: slot(regs, *a)?,
            b: slot(regs, *b)?,
            op: *op,
        },
        Op::BinImm { dst, a, imm, op } => LOp::BinImm {
            dst: slot(regs, *dst)?,
            a: slot(regs, *a)?,
            imm: *imm,
            op: *op,
        },
        Op::Un { dst, a, op } => LOp::Un {
            dst: slot(regs, *dst)?,
            a: slot(regs, *a)?,
            op: *op,
        },
        Op::Jump { to } => LOp::Jump {
            to: target(head, exit, *to)?,
        },
        Op::JumpIfFalse { cond, to } => LOp::JumpIfFalse {
            cond: slot(regs, *cond)?,
            to: target(head, exit, *to)?,
        },
        Op::JumpIfTrue { cond, to } => LOp::JumpIfTrue {
            cond: slot(regs, *cond)?,
            to: target(head, exit, *to)?,
        },
        Op::CmpJump { a, b, op, to } => LOp::CmpJump {
            a: slot(regs, *a)?,
            b: slot(regs, *b)?,
            op: *op,
            to: target(head, exit, *to)?,
        },
        Op::CmpJumpImm { a, imm, op, to } => LOp::CmpJumpImm {
            a: slot(regs, *a)?,
            imm: *imm,
            op: *op,
            to: target(head, exit, *to)?,
        },
        Op::Cast { dst, src, ty } => match chunk.casts[*ty as usize] {
            CastIr::Int(w) if !w.is_big() => LOp::Cast {
                dst: slot(regs, *dst)?,
                src: slot(regs, *src)?,
                w,
            },
            _ => return None,
        },
        _ => return None,
    })
}

/// The one slot an op writes, for the move-folding pass. Jumps write none.
fn op_write(op: &LOp) -> Option<u16> {
    match op {
        LOp::LoadUnit { dst }
        | LOp::LoadInt { dst, .. }
        | LOp::LoadIntW { dst, .. }
        | LOp::LoadBool { dst, .. }
        | LOp::Move { dst, .. }
        | LOp::Bin { dst, .. }
        | LOp::BinImm { dst, .. }
        | LOp::Un { dst, .. }
        | LOp::Cast { dst, .. } => Some(*dst),
        _ => None,
    }
}

/// Every slot an op reads, for the move-folding pass.
fn op_reads(op: &LOp, mut read: impl FnMut(u16)) {
    match op {
        LOp::Move { src, .. } | LOp::Un { a: src, .. } | LOp::Cast { src, .. } => read(*src),
        LOp::Bin { a, b, .. } | LOp::CmpJump { a, b, .. } => {
            read(*a);
            read(*b);
        }
        LOp::BinImm { a, .. } | LOp::CmpJumpImm { a, .. } => read(*a),
        LOp::JumpIfFalse { cond, .. } | LOp::JumpIfTrue { cond, .. } => read(*cond),
        LOp::LoadUnit { .. }
        | LOp::LoadInt { .. }
        | LOp::LoadIntW { .. }
        | LOp::LoadBool { .. }
        | LOp::Jump { .. } => {}
    }
}

/// Retarget an op's write, for the move-folding pass.
fn set_write(op: &mut LOp, to: u16) {
    match op {
        LOp::LoadUnit { dst }
        | LOp::LoadInt { dst, .. }
        | LOp::LoadIntW { dst, .. }
        | LOp::LoadBool { dst, .. }
        | LOp::Move { dst, .. }
        | LOp::Bin { dst, .. }
        | LOp::BinImm { dst, .. }
        | LOp::Un { dst, .. }
        | LOp::Cast { dst, .. } => *dst = to,
        _ => unreachable!("only value ops fold"),
    }
}

/// Fold `op -> Move` pairs where the op's destination is an expression
/// temporary: written only by that op and read only by that move. The
/// compiler never reuses a register, so such a temporary is dead once the
/// move consumed it, and the producing op can write the move's destination
/// directly. Also drops constant loads into registers nothing in the whole
/// chunk reads, the per-statement unit results. Runs to a fixpoint so a
/// chain of moves collapses.
fn fold_moves(ops: &mut Vec<LOp>, val_slot: u16, frame_read: &[bool], slot_regs: &[u16]) {
    loop {
        let mut writes = vec![0u32; MAX_SLOTS];
        let mut reads = vec![0u32; MAX_SLOTS];
        let mut targets = vec![false; ops.len() + 1];
        for op in ops.iter() {
            if let Some(dst) = op_write(op) {
                writes[usize::from(dst)] += 1;
            }
            op_reads(op, |r| reads[usize::from(r)] += 1);
            let jump_to = match op {
                LOp::Jump { to }
                | LOp::JumpIfFalse { to, .. }
                | LOp::JumpIfTrue { to, .. }
                | LOp::CmpJump { to, .. }
                | LOp::CmpJumpImm { to, .. } => Some(to),
                _ => None,
            };
            if let Some(LTo::Op(t)) = jump_to {
                targets[*t as usize] = true;
            }
        }
        let foldable = |i: usize, ops: &[LOp]| {
            let LOp::Move { dst, src } = ops[i + 1] else {
                return None;
            };
            let temp = op_write(&ops[i])?;
            let ok = temp == src
                && temp != dst
                && temp != val_slot
                && writes[usize::from(temp)] == 1
                && reads[usize::from(temp)] == 1
                && !targets[i + 1];
            ok.then_some(dst)
        };
        if let Some((at, dst)) =
            (0..ops.len().saturating_sub(1)).find_map(|i| foldable(i, ops).map(|dst| (i, dst)))
        {
            set_write(&mut ops[at], dst);
            remove_op(ops, at + 1);
            continue;
        }
        // A constant load into a register nothing in the plan and nothing in
        // the whole chunk reads is a dead store, the per-statement unit
        // results. Jumps that targeted it run its successor, which is what
        // executing a dead store followed by the successor did.
        let dead = |i: &usize| {
            let op = &ops[*i];
            let constant = matches!(
                op,
                LOp::LoadUnit { .. }
                    | LOp::LoadInt { .. }
                    | LOp::LoadIntW { .. }
                    | LOp::LoadBool { .. }
            );
            constant
                && op_write(op).is_some_and(|dst| {
                    reads[usize::from(dst)] == 0
                        && !frame_read
                            .get(usize::from(slot_regs[usize::from(dst)]))
                            .copied()
                            .unwrap_or(true)
                })
        };
        let Some(at) = (0..ops.len()).find(dead) else {
            return;
        };
        remove_op(ops, at);
    }
}

/// Remove one op, sliding every jump target past it down one.
fn remove_op(ops: &mut Vec<LOp>, at: usize) {
    ops.remove(at);
    for op in ops.iter_mut() {
        let (LOp::Jump { to }
        | LOp::JumpIfFalse { to, .. }
        | LOp::JumpIfTrue { to, .. }
        | LOp::CmpJump { to, .. }
        | LOp::CmpJumpImm { to, .. }) = op
        else {
            continue;
        };
        if let LTo::Op(t) = to
            && *t as usize > at
        {
            *to = LTo::Op(*t - 1);
        }
    }
}

/// Translate the body of the `for` loop whose `ForNext` sits at `head`, or
/// answer None when any op falls outside the subset.
fn build(chunk: &Chunk, head: usize) -> Option<LoopPlan> {
    let Some(Op::ForNext { val, to, .. }) = chunk.code.get(head) else {
        return None;
    };
    let exit = *to as usize;
    if exit <= head + 1 || exit > chunk.code.len() {
        return None;
    }
    let mut regs: Vec<u16> = Vec::new();
    let val_slot = slot(&mut regs, *val)?;
    let mut ops = chunk.code[head + 1..exit]
        .iter()
        .map(|op| translate(chunk, head, exit, &mut regs, op))
        .collect::<Option<Vec<_>>>()?;
    fold_moves(&mut ops, val_slot, &chunk_reads(chunk), &regs);
    let straight = ops.iter().enumerate().all(|(i, op)| match op {
        LOp::Jump { to: LTo::Next } => i == ops.len() - 1,
        LOp::Jump { .. }
        | LOp::JumpIfFalse { .. }
        | LOp::JumpIfTrue { .. }
        | LOp::CmpJump { .. }
        | LOp::CmpJumpImm { .. } => false,
        _ => true,
    });
    // A straight body's trailing back jump is implicit in the slice walk.
    if straight && matches!(ops.last(), Some(LOp::Jump { to: LTo::Next })) {
        ops.pop();
    }
    Some(LoopPlan {
        ops,
        regs,
        val_slot,
        straight,
    })
}

enum BodyOut {
    Next,
    Exit,
    Fail,
}

/// What one op did: fall through, take a jump, or fail the iteration.
enum OpOut {
    Fall,
    Jump(LTo),
    Fail,
}

#[inline]
fn eval_op(op: &LOp, regs: &mut [SVal]) -> OpOut {
    match op {
        LOp::LoadUnit { dst } => regs[usize::from(*dst)] = SVal::Unit,
        LOp::LoadInt { dst, v } => regs[usize::from(*dst)] = SVal::Int(*v),
        LOp::LoadIntW { dst, v, w } => regs[usize::from(*dst)] = SVal::IntW(*v, *w),
        LOp::LoadBool { dst, v } => regs[usize::from(*dst)] = SVal::Bool(*v),
        LOp::Move { dst, src } => regs[usize::from(*dst)] = regs[usize::from(*src)],
        LOp::Bin { dst, a, b, op } => {
            let (x, y) = (regs[usize::from(*a)], regs[usize::from(*b)]);
            match s_bin(*op, x, y) {
                Some(v) => regs[usize::from(*dst)] = v,
                None => return OpOut::Fail,
            }
        }
        LOp::BinImm { dst, a, imm, op } => {
            let x = regs[usize::from(*a)];
            match s_bin(*op, x, SVal::Int(*imm)) {
                Some(v) => regs[usize::from(*dst)] = v,
                None => return OpOut::Fail,
            }
        }
        LOp::Un { dst, a, op } => match s_un(*op, regs[usize::from(*a)]) {
            Some(v) => regs[usize::from(*dst)] = v,
            None => return OpOut::Fail,
        },
        LOp::Jump { to } => return OpOut::Jump(*to),
        LOp::JumpIfFalse { cond, to } => {
            if matches!(regs[usize::from(*cond)], SVal::Opaque) {
                return OpOut::Fail;
            }
            if !truthy(regs[usize::from(*cond)]) {
                return OpOut::Jump(*to);
            }
        }
        LOp::JumpIfTrue { cond, to } => {
            if matches!(regs[usize::from(*cond)], SVal::Opaque) {
                return OpOut::Fail;
            }
            if truthy(regs[usize::from(*cond)]) {
                return OpOut::Jump(*to);
            }
        }
        LOp::CmpJump { a, b, op, to } => {
            let (x, y) = (regs[usize::from(*a)], regs[usize::from(*b)]);
            match s_cmp(*op, x, y) {
                Some(true) => {}
                Some(false) => return OpOut::Jump(*to),
                None => return OpOut::Fail,
            }
        }
        LOp::CmpJumpImm { a, imm, op, to } => {
            let x = regs[usize::from(*a)];
            match s_cmp(*op, x, SVal::Int(*imm)) {
                Some(true) => {}
                Some(false) => return OpOut::Jump(*to),
                None => return OpOut::Fail,
            }
        }
        LOp::Cast { dst, src, w } => match s_cast(regs[usize::from(*src)], *w) {
            Some(v) => regs[usize::from(*dst)] = v,
            None => return OpOut::Fail,
        },
    }
    OpOut::Fall
}

/// Run the body once for one item. `Fail` leaves the registers mid-body,
/// the caller restores them by replaying the chunk snapshot.
#[inline]
fn run_body(plan: &LoopPlan, regs: &mut [SVal], item: i64) -> BodyOut {
    regs[usize::from(plan.val_slot)] = SVal::Int(item);
    if plan.straight {
        // One basic block with the back jump trimmed: walk the slice with no
        // instruction pointer, the end of the slice is the next iteration.
        for op in &plan.ops {
            match eval_op(op, regs) {
                OpOut::Fall => {}
                OpOut::Jump(_) | OpOut::Fail => return BodyOut::Fail,
            }
        }
        return BodyOut::Next;
    }
    let mut ip = 0usize;
    let mut steps = 0u32;
    loop {
        let Some(op) = plan.ops.get(ip) else {
            return BodyOut::Fail;
        };
        match eval_op(op, regs) {
            OpOut::Fall => ip += 1,
            OpOut::Fail => return BodyOut::Fail,
            OpOut::Jump(LTo::Next) => return BodyOut::Next,
            OpOut::Jump(LTo::Exit) => return BodyOut::Exit,
            OpOut::Jump(LTo::Op(t)) => {
                let t = t as usize;
                // The budget counts backward jumps, the one way an iteration
                // can run long, so straight runs pay no counter.
                if t <= ip {
                    steps += 1;
                    if steps > MAX_BODY_STEPS {
                        return BodyOut::Fail;
                    }
                }
                ip = t;
            }
        }
    }
}

/// Rebuild the registers to the state at the start of the failing item:
/// restore the chunk snapshot, then re-run the items that succeeded. The
/// body only touches registers, so the replay is deterministic.
fn replay(
    plan: &LoopPlan,
    regs: &mut [SVal],
    snapshot: &[SVal],
    item: impl Fn(usize) -> i64,
    count: usize,
) {
    regs.copy_from_slice(snapshot);
    for k in 0..count {
        run_body(plan, regs, item(k));
    }
}

/// What one locked chunk of items did, plus how many items it consumed.
struct ChunkOut {
    advanced: i64,
    state: ChunkState,
}

enum ChunkState {
    /// The source is exhausted.
    Done,
    /// The body hit a `break`.
    Exited,
    /// An iteration failed, its item is unconsumed and the registers hold
    /// its entry state.
    Failed,
    /// The chunk filled up, more items remain.
    More,
    /// The iterator is not a supported simple source.
    NotSimple,
}

fn bytes_chunk(
    plan: &LoopPlan,
    regs: &mut [SVal],
    snapshot: &mut Vec<SVal>,
    source: &str,
    index: &mut usize,
) -> ChunkOut {
    snapshot.clear();
    snapshot.extend_from_slice(regs);
    let bytes = source.as_bytes();
    let start = *index;
    let mut advanced = 0i64;
    let out = |advanced, state| ChunkOut { advanced, state };
    for _ in 0..CHUNK {
        let Some(&b) = bytes.get(*index) else {
            return out(advanced, ChunkState::Done);
        };
        match run_body(plan, regs, i64::from(b)) {
            BodyOut::Next => {
                *index += 1;
                advanced += 1;
            }
            BodyOut::Exit => {
                *index += 1;
                advanced += 1;
                return out(advanced, ChunkState::Exited);
            }
            BodyOut::Fail => {
                replay(
                    plan,
                    regs,
                    snapshot,
                    |k| i64::from(bytes[start + k]),
                    *index - start,
                );
                return out(advanced, ChunkState::Failed);
            }
        }
    }
    out(advanced, ChunkState::More)
}

fn range_chunk(
    plan: &LoopPlan,
    regs: &mut [SVal],
    snapshot: &mut Vec<SVal>,
    next: &mut i64,
    end: i64,
    inclusive: bool,
) -> ChunkOut {
    snapshot.clear();
    snapshot.extend_from_slice(regs);
    let start = *next;
    let mut advanced = 0i64;
    let out = |advanced, state| ChunkOut { advanced, state };
    for _ in 0..CHUNK {
        let done = if inclusive { *next > end } else { *next >= end };
        if done {
            return out(advanced, ChunkState::Done);
        }
        let item = *next;
        match run_body(plan, regs, item) {
            BodyOut::Next => {
                // Wrapping mirrors the release-built generic `range_step`,
                // whose bare `+= 1` wraps at the inclusive i64::MAX end.
                *next = next.wrapping_add(1);
                advanced += 1;
            }
            BodyOut::Exit => {
                *next = next.wrapping_add(1);
                advanced += 1;
                return out(advanced, ChunkState::Exited);
            }
            BodyOut::Fail => {
                let count = usize::try_from(advanced).unwrap_or(0);
                replay(
                    plan,
                    regs,
                    snapshot,
                    |k| start.wrapping_add(usize_i64(k)),
                    count,
                );
                return out(advanced, ChunkState::Failed);
            }
        }
    }
    out(advanced, ChunkState::More)
}

fn usize_i64(v: usize) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn write_back(ctx: &mut StepCtx, plan: &LoopPlan, regs: &[SVal], idx: u16, consumed: i64) {
    for (slot, &reg) in plan.regs.iter().enumerate() {
        match regs[slot] {
            SVal::Opaque => {}
            SVal::Unit => ctx.put(reg, Value::Unit),
            SVal::Int(i) => ctx.put(reg, Value::Int(i)),
            SVal::IntW(s, w) => ctx.put(reg, Value::IntW(s, w)),
            SVal::Bool(b) => ctx.put(reg, Value::Bool(b)),
        }
    }
    ctx.put(idx, Value::Int(consumed));
}

/// Try to run the whole loop at the `ForNext` under `ctx.ip` as a scalar
/// plan. `None` means the generic path should run, with the frame and the
/// iterator left exactly where a generic execution would have them.
pub(super) fn try_run(ctx: &mut StepCtx, iter: u16, idx: u16, to: u32) -> Result<Option<Flow>> {
    let head = ctx.ip;
    let plan = {
        let mut plans = ctx.cur.loop_plans.lock();
        if let Some(cached) = plans.get(&head) {
            cached.clone()
        } else {
            let built = build(ctx.cur, head).map(Arc::new);
            plans.insert(head, built.clone());
            built
        }
    };
    let Some(plan) = plan else { return Ok(None) };
    let Value::Native(handle) = ctx.get(iter) else {
        return Ok(None);
    };
    let handle = handle.clone();
    let mut regs: Vec<SVal> = plan.regs.iter().map(|&r| SVal::of(ctx.get(r))).collect();
    let mut snapshot: Vec<SVal> = Vec::with_capacity(regs.len());
    let mut consumed = 0i64;
    loop {
        let out = {
            let mut native = handle.lock();
            match &mut *native {
                Native::Iterator(IteratorState::Bytes { source, index }) => {
                    bytes_chunk(&plan, &mut regs, &mut snapshot, source, index)
                }
                Native::Iterator(IteratorState::Range {
                    next,
                    end,
                    inclusive,
                }) => range_chunk(&plan, &mut regs, &mut snapshot, next, *end, *inclusive),
                _ => ChunkOut {
                    advanced: 0,
                    state: ChunkState::NotSimple,
                },
            }
        };
        consumed += out.advanced;
        match out.state {
            ChunkState::NotSimple if consumed == 0 => return Ok(None),
            ChunkState::NotSimple | ChunkState::Failed => {
                write_back(ctx, &plan, &regs, idx, consumed);
                return Ok(None);
            }
            ChunkState::Done | ChunkState::Exited => {
                write_back(ctx, &plan, &regs, idx, consumed);
                return Ok(Some(Flow::Jump(to as usize)));
            }
            ChunkState::More => {
                write_back(ctx, &plan, &regs, idx, consumed);
                ctx.vm.run_pending_ctrlc()?;
            }
        }
    }
}
