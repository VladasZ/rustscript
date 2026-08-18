//! The `for` side of the scalar loop specialization: run the whole loop at
//! a `ForNext` as a plan over unboxed registers, in chunks so the iterator
//! lock drops and a pending Ctrl-C handler runs between them. The plan IR,
//! its translation, and the op evaluator live in `scalar_loop`.

use std::sync::Arc;

use anyhow::Result;
use parking_lot::Mutex;

use super::iterator::{IteratorState, regex_find_span};
use super::native::Native;
use super::regex_bridge::{RegexValue, match_value};
use super::scalar_loop::{LTo, LoopPlan, OpOut, build, eval_op, write_regs};
use super::scalar_val::SVal;
use super::value::{RsStr, Value};
use super::vm_step::{Flow, StepCtx};

type Handle = Arc<Mutex<Native>>;

/// Items processed per iterator lock hold. Between chunks the lock drops,
/// the registers write back, and the pending Ctrl-C handler runs, so a long
/// loop cannot starve either.
const CHUNK: usize = 4096;

/// Op budget for one iteration, so an inner loop that runs long falls back
/// to the generic path, which polls Ctrl-C on every backward jump.
const MAX_BODY_STEPS: u32 = 65_536;

enum BodyOut {
    Next,
    Exit,
    Fail,
}

/// Run the body once for one item. `Fail` leaves the registers mid-body,
/// the caller restores them by replaying the chunk snapshot.
#[inline]
fn run_body(plan: &LoopPlan, regs: &mut [SVal], item: SVal) -> BodyOut {
    regs[usize::from(plan.val_slot)] = item;
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
    item: impl Fn(usize) -> SVal,
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
        match run_body(plan, regs, SVal::Int(i64::from(b))) {
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
                    |k| SVal::Int(i64::from(bytes[start + k])),
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
        match run_body(plan, regs, SVal::Int(item)) {
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
                    |k| SVal::Int(start.wrapping_add(usize_i64(k))),
                    count,
                );
                return out(advanced, ChunkState::Failed);
            }
        }
    }
    out(advanced, ChunkState::More)
}

/// A chunk over a vec's elements, `Values` locked in the caller or `Owned`
/// held by the iterator itself. A non-scalar element fails its iteration
/// over unconsumed, so the generic loop binds and runs it.
fn values_chunk(
    plan: &LoopPlan,
    regs: &mut [SVal],
    snapshot: &mut Vec<SVal>,
    items: &[Value],
    index: &mut usize,
) -> ChunkOut {
    snapshot.clear();
    snapshot.extend_from_slice(regs);
    let start = *index;
    let mut advanced = 0i64;
    let out = |advanced, state| ChunkOut { advanced, state };
    for _ in 0..CHUNK {
        let Some(item) = items.get(*index) else {
            return out(advanced, ChunkState::Done);
        };
        let item = SVal::of(item);
        let fail = |regs: &mut [SVal]| {
            replay(
                plan,
                regs,
                snapshot,
                |k| SVal::of(&items[start + k]),
                *index - start,
            );
            out(advanced, ChunkState::Failed)
        };
        if matches!(item, SVal::Opaque) {
            return fail(regs);
        }
        match run_body(plan, regs, item) {
            BodyOut::Next => {
                *index += 1;
                advanced += 1;
            }
            BodyOut::Exit => {
                *index += 1;
                advanced += 1;
                return out(advanced, ChunkState::Exited);
            }
            BodyOut::Fail => return fail(regs),
        }
    }
    out(advanced, ChunkState::More)
}

/// A chunk over `find_iter` matches, each item a `Span` over the locked
/// source. A failing iteration rewinds the offset to before its match, so
/// the generic `ForNext` re-pulls the same match through the same shared
/// span walk.
fn regex_chunk(
    plan: &LoopPlan,
    regs: &mut [SVal],
    snapshot: &mut Vec<SVal>,
    regex: &RegexValue,
    source: &RsStr,
    offset: &mut usize,
) -> ChunkOut {
    snapshot.clear();
    snapshot.extend_from_slice(regs);
    let mut items: Vec<SVal> = Vec::new();
    let mut advanced = 0i64;
    let out = |advanced, state| ChunkOut { advanced, state };
    let fail = |regs: &mut [SVal], items: &[SVal], advanced| {
        replay(plan, regs, snapshot, |k| items[k], items.len());
        out(advanced, ChunkState::Failed)
    };
    for _ in 0..CHUNK {
        let before = *offset;
        let Some((start, end)) = regex_find_span(regex, source, offset) else {
            return out(advanced, ChunkState::Done);
        };
        // A span past the u32 range has no slot form, so its iteration
        // fails over unconsumed and the generic loop binds the real match.
        let (Ok(start), Ok(end)) = (u32::try_from(start), u32::try_from(end)) else {
            *offset = before;
            return fail(regs, &items, advanced);
        };
        let item = SVal::Span { start, end };
        match run_body(plan, regs, item) {
            BodyOut::Next => {
                advanced += 1;
                items.push(item);
            }
            BodyOut::Exit => {
                advanced += 1;
                return out(advanced, ChunkState::Exited);
            }
            BodyOut::Fail => {
                *offset = before;
                return fail(regs, &items, advanced);
            }
        }
    }
    out(advanced, ChunkState::More)
}

/// If the iterator is a `skip` over a simple vec source, fold the pending
/// skip into the inner index once and answer the inner handle, so the
/// chunks run on the source directly. The generic path sees exactly the
/// state its own pulls would leave: the skip spent, the inner advanced.
fn resolve_skip(handle: &Handle) -> Handle {
    let mut native = handle.lock();
    let Native::Iterator(IteratorState::Skip { source, remaining }) = &mut *native else {
        drop(native);
        return handle.clone();
    };
    let inner = source.clone();
    {
        let mut inner_native = inner.lock();
        match &mut *inner_native {
            Native::Iterator(IteratorState::Owned { values, index }) => {
                *index = (*index + *remaining).min(values.len());
            }
            Native::Iterator(IteratorState::Values { values, index }) => {
                let len = values.lock().len();
                *index = (*index + *remaining).min(len);
            }
            _ => {
                drop(inner_native);
                drop(native);
                return handle.clone();
            }
        }
    }
    *remaining = 0;
    inner
}

fn usize_i64(v: usize) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn write_back(
    ctx: &mut StepCtx,
    plan: &LoopPlan,
    regs: &[SVal],
    idx: u16,
    consumed: i64,
    match_source: Option<&RsStr>,
) {
    // A `Span` slot has no boxed form of its own, `s_value` skips it, so
    // its register gets the real match value here, source included, exactly
    // what the generic `ForNext` would have bound.
    if let Some(source) = match_source {
        for (slot, sval) in regs.iter().enumerate() {
            if let SVal::Span { start, end } = *sval {
                let start = usize::try_from(start).expect("u32 fits usize");
                let end = usize::try_from(end).expect("u32 fits usize");
                ctx.put(plan.regs[slot], match_value(source.clone(), start, end));
            }
        }
    }
    write_regs(ctx, &plan.regs, regs);
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
            let built = build(ctx.vm, ctx.cur, head).map(Arc::new);
            plans.insert(head, built.clone());
            built
        }
    };
    let Some(plan) = plan else { return Ok(None) };
    let Value::Native(handle) = ctx.get(iter) else {
        return Ok(None);
    };
    let handle = resolve_skip(&handle.clone());
    let mut regs: Vec<SVal> = plan.regs.iter().map(|&r| SVal::of(ctx.get(r))).collect();
    let mut snapshot: Vec<SVal> = Vec::with_capacity(regs.len());
    let mut consumed = 0i64;
    let mut match_source: Option<RsStr> = None;
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
                Native::Iterator(IteratorState::Owned { values, index }) => {
                    values_chunk(&plan, &mut regs, &mut snapshot, values, index)
                }
                Native::Iterator(IteratorState::Values { values, index }) => {
                    let items = values.lock();
                    values_chunk(&plan, &mut regs, &mut snapshot, &items, index)
                }
                Native::Iterator(IteratorState::RegexFind {
                    regex,
                    source,
                    offset,
                }) => {
                    if match_source.is_none() {
                        match_source = Some(source.clone());
                    }
                    regex_chunk(&plan, &mut regs, &mut snapshot, regex, source, offset)
                }
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
                write_back(ctx, &plan, &regs, idx, consumed, match_source.as_ref());
                return Ok(None);
            }
            ChunkState::Done | ChunkState::Exited => {
                write_back(ctx, &plan, &regs, idx, consumed, match_source.as_ref());
                return Ok(Some(Flow::Jump(to as usize)));
            }
            ChunkState::More => {
                write_back(ctx, &plan, &regs, idx, consumed, match_source.as_ref());
                ctx.vm.run_pending_ctrlc()?;
            }
        }
    }
}
