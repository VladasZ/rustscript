//! The `for` side of the scalar loop specialization: run the whole loop at
//! a `ForNext` as a plan over unboxed registers, in chunks so the iterator
//! lock drops and a pending Ctrl-C handler runs between them. The plan IR,
//! its translation, and the op evaluator live in `scalar_loop`.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use anyhow::Result;
use parking_lot::{Mutex, MutexGuard};

use super::iterator::{IteratorState, next_word_span, regex_find_span};
use super::native::Native;
use super::regex_bridge::{RegexValue, match_value};
use super::scalar_loop::{
    LOp, LTo, LoopPlan, MAX_PUSH_VECS, NO_SLOT, OpOut, build, eval_op, write_regs,
};
use super::scalar_val::{SVal, s_map_key, s_value};
use super::value::{List, Map, MapKey, MapKind, RsStr, StrKey, Value};
use super::vecmap::MapStore;
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

/// A chunk over `split_whitespace` words, each item a `StrSpan` over the
/// locked source. A failing iteration rewinds the offset to before its
/// word, so the generic `ForNext` re-pulls the same word through the same
/// shared span walk.
fn words_chunk(
    plan: &LoopPlan,
    regs: &mut [SVal],
    snapshot: &mut Vec<SVal>,
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
        let Some((start, end)) = next_word_span(source, offset) else {
            return out(advanced, ChunkState::Done);
        };
        // A word past the u32 range has no slot form, so its iteration
        // fails over unconsumed and the generic loop binds the real slice.
        let (Ok(start), Ok(end)) = (u32::try_from(start), u32::try_from(end)) else {
            *offset = before;
            return fail(regs, &items, advanced);
        };
        let item = SVal::StrSpan { start, end };
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
    span_source: Option<&RsStr>,
) {
    // A span slot has no boxed form of its own, `s_value` skips it, so its
    // register gets the real value here, a match with its source for a
    // `Span` and the owned slice for a `StrSpan`, exactly what the generic
    // `ForNext` and the generic method would have bound.
    if let Some(source) = span_source {
        for (slot, sval) in regs.iter().enumerate() {
            match *sval {
                SVal::Span { start, end } => {
                    let start = usize::try_from(start).expect("u32 fits usize");
                    let end = usize::try_from(end).expect("u32 fits usize");
                    ctx.put(plan.regs[slot], match_value(source.clone(), start, end));
                }
                SVal::StrSpan { start, end } => {
                    let start = usize::try_from(start).expect("u32 fits usize");
                    let end = usize::try_from(end).expect("u32 fits usize");
                    ctx.put(plan.regs[slot], Value::str(&source[start..end]));
                }
                _ => {}
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
    if !plan.vecs.is_empty() || !plan.maps.is_empty() || plan.needs_items {
        return run_effects(ctx, &plan, head, iter, idx, to);
    }
    let Value::Native(handle) = ctx.get(iter) else {
        return Ok(None);
    };
    let handle = resolve_skip(&handle.clone());
    let mut regs: Vec<SVal> = plan.regs.iter().map(|&r| SVal::of(ctx.get(r))).collect();
    let mut snapshot: Vec<SVal> = Vec::with_capacity(regs.len());
    let mut consumed = 0i64;
    let mut span_source: Option<RsStr> = None;
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
                Native::Iterator(IteratorState::SplitWhitespace { source, offset }) => {
                    if span_source.is_none() {
                        span_source = Some(source.clone());
                    }
                    words_chunk(&plan, &mut regs, &mut snapshot, source, offset)
                }
                Native::Iterator(IteratorState::RegexFind {
                    regex,
                    source,
                    offset,
                }) => {
                    if span_source.is_none() {
                        span_source = Some(source.clone());
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
                write_back(ctx, &plan, &regs, idx, consumed, span_source.as_ref());
                return Ok(None);
            }
            ChunkState::Done | ChunkState::Exited => {
                write_back(ctx, &plan, &regs, idx, consumed, span_source.as_ref());
                return Ok(Some(Flow::Jump(to as usize)));
            }
            ChunkState::More => {
                write_back(ctx, &plan, &regs, idx, consumed, span_source.as_ref());
                ctx.vm.run_pending_ctrlc()?;
            }
        }
    }
}

/// One journaled map insert of the current iteration, undone newest first
/// on failure so a doubly-written key ends on its original value.
struct MapUndo {
    map: u16,
    key: MapKey,
    old: Option<Value>,
}

/// The locked effect state of one chunk: the vec and map storage guards,
/// the current iteration's map journal, and the span source of the loop's
/// items, which resolves span slots into the string keys they stand for.
struct Effects<'g, 'v> {
    vecs: &'g mut [MutexGuard<'v, Vec<Value>>],
    maps: &'g mut [MutexGuard<'v, MapStore>],
    /// The arcs behind `maps`, for the `ItemIndex` alias check: probing an
    /// item that IS a locked store would self-deadlock, so it fails over.
    stores: &'g [Map],
    journal: &'g mut Vec<MapUndo>,
    source: Option<&'g RsStr>,
    strs: &'g [Box<str>],
}

/// A map probe's key: an owned scalar key, or the borrowed slice a span
/// slot names in the loop's source. `None` sends the access to the generic
/// path, which reproduces the exact error for a value that cannot be a key.
enum ProbeKey<'a> {
    Owned(MapKey),
    Slice(&'a str),
}

/// Resolve a key slot against the span source and the plan's string table,
/// see `ProbeKey`.
fn probe_key<'a>(v: SVal, source: Option<&'a RsStr>, strs: &'a [Box<str>]) -> Option<ProbeKey<'a>> {
    match v {
        SVal::Span { start, end } | SVal::StrSpan { start, end } => {
            let source = source?;
            let start = usize::try_from(start).expect("u32 fits usize");
            let end = usize::try_from(end).expect("u32 fits usize");
            Some(ProbeKey::Slice(&source[start..end]))
        }
        SVal::StrConst(id) => Some(ProbeKey::Slice(&strs[usize::from(id)])),
        other => s_map_key(other).map(ProbeKey::Owned),
    }
}

/// The `MapGetOr` arm of `run_body_effects`, the fused
/// `map.get(k).copied().unwrap_or(d)`. A missing key answers the default
/// slot, whose `Opaque` fails over the same way an unreadable hit does.
#[inline]
fn map_get_or(
    regs: &mut [SVal],
    fx: &mut Effects<'_, '_>,
    dst: u16,
    map: u16,
    key: u16,
    default: u16,
) -> bool {
    let Some(k) = probe_key(regs[usize::from(key)], fx.source, fx.strs) else {
        return false;
    };
    let store = &fx.maps[usize::from(map)];
    let hit = match k {
        ProbeKey::Owned(k) => store.get(&k),
        ProbeKey::Slice(text) => store.get(&StrKey(text)),
    };
    let v = match hit {
        Some(hit) => SVal::of(hit),
        None => regs[usize::from(default)],
    };
    if matches!(v, SVal::Opaque) {
        return false;
    }
    regs[usize::from(dst)] = v;
    true
}

/// The `MapGetOpt` arm of `run_body_effects`: `map.get(&k)` as a `SomeInt`
/// or `NoneOpt` slot. A hit whose value is not a plain int fails over, the
/// slot has no form for it.
#[inline]
fn map_get_opt(regs: &mut [SVal], fx: &mut Effects<'_, '_>, dst: u16, map: u16, key: u16) -> bool {
    let Some(k) = probe_key(regs[usize::from(key)], fx.source, fx.strs) else {
        return false;
    };
    let store = &fx.maps[usize::from(map)];
    let hit = match k {
        ProbeKey::Owned(k) => store.get(&k),
        ProbeKey::Slice(text) => store.get(&StrKey(text)),
    };
    regs[usize::from(dst)] = match hit {
        Some(Value::Int(n)) => SVal::SomeInt(*n),
        Some(_) => return false,
        None => SVal::NoneOpt,
    };
    true
}

/// The `MapHas` arm of `run_body_effects`.
#[inline]
fn map_has(regs: &mut [SVal], fx: &mut Effects<'_, '_>, dst: u16, map: u16, key: u16) -> bool {
    let Some(k) = probe_key(regs[usize::from(key)], fx.source, fx.strs) else {
        return false;
    };
    let store = &fx.maps[usize::from(map)];
    let found = match k {
        ProbeKey::Owned(k) => store.contains_key(&k),
        ProbeKey::Slice(text) => store.contains_key(&StrKey(text)),
    };
    regs[usize::from(dst)] = SVal::Bool(found);
    true
}

/// The `MapInsert` arm of `run_body_effects`, journaled so a failing
/// iteration can undo it. A kept old value that is not a plain int fails
/// over after the journal entry lands, so the undo still restores it.
#[inline]
fn map_insert(
    regs: &mut [SVal],
    fx: &mut Effects<'_, '_>,
    dst: u16,
    map: u16,
    key: u16,
    val: u16,
) -> bool {
    let (Some(k), Some(v)) = (
        probe_key(regs[usize::from(key)], fx.source, fx.strs),
        s_value(regs[usize::from(val)]),
    ) else {
        return false;
    };
    // The owned key an insert needs: a span key materializes here, the
    // same one string copy the generic `to_string` made.
    let k = match k {
        ProbeKey::Owned(k) => k,
        ProbeKey::Slice(text) => MapKey::Str(RsStr::from(text)),
    };
    let old = fx.maps[usize::from(map)].insert(k.clone(), v);
    let kept = match &old {
        None => SVal::NoneOpt,
        Some(Value::Int(n)) => SVal::SomeInt(*n),
        Some(_) => SVal::Opaque,
    };
    fx.journal.push(MapUndo { map, key: k, old });
    if dst != NO_SLOT {
        if matches!(kept, SVal::Opaque) {
            return false;
        }
        regs[usize::from(dst)] = kept;
    }
    true
}

/// The `ItemIndex` arm of `run_body_effects`: probe the boxed source item
/// an `Item` slot points at, the json shape of `it["key"]`. The item map's
/// own lock is taken per probe. A non-map item, a missing key, whose
/// generic twin is an error, or a non-scalar hit fails the iteration over.
#[inline]
fn item_index(
    regs: &mut [SVal],
    fx: &mut Effects<'_, '_>,
    items: Option<&[Value]>,
    dst: u16,
    item: u16,
    key: u16,
) -> bool {
    let SVal::Item(idx) = regs[usize::from(item)] else {
        return false;
    };
    let Some(entry) =
        items.and_then(|items| items.get(usize::try_from(idx).expect("u32 fits usize")))
    else {
        return false;
    };
    let Value::Map(m, MapKind::Map) = entry else {
        return false;
    };
    if fx.stores.iter().any(|store| Arc::ptr_eq(store, m)) {
        return false;
    }
    let Some(k) = probe_key(regs[usize::from(key)], fx.source, fx.strs) else {
        return false;
    };
    let store = m.lock();
    let hit = match k {
        ProbeKey::Owned(k) => store.get(&k),
        ProbeKey::Slice(text) => store.get(&StrKey(text)),
    };
    let v = match hit {
        Some(hit) => SVal::of(hit),
        None => return false,
    };
    drop(store);
    if matches!(v, SVal::Opaque) {
        return false;
    }
    regs[usize::from(dst)] = v;
    true
}

/// Restore this iteration's map writes newest first. A fresh insert
/// appended at its map's end and every later append was already undone, so
/// its undo pops the tail; a replacement re-inserts in place.
fn unwind_maps(fx: &mut Effects<'_, '_>) {
    while let Some(MapUndo { map, key, old }) = fx.journal.pop() {
        let store = &mut fx.maps[usize::from(map)];
        match old {
            Some(v) => {
                store.insert(key, v);
            }
            None => {
                store.pop();
            }
        }
    }
}

/// One iteration of an effects plan. Failure recovery is the caller's: it
/// restores the register snapshot, truncates each vec to its entry length,
/// and unwinds the map journal, so it needs no replay.
#[inline]
fn run_body_effects(
    plan: &LoopPlan,
    regs: &mut [SVal],
    item: SVal,
    fx: &mut Effects<'_, '_>,
    items: Option<&[Value]>,
) -> BodyOut {
    regs[usize::from(plan.val_slot)] = item;
    let mut ip = 0usize;
    let mut steps = 0u32;
    loop {
        let Some(op) = plan.ops.get(ip) else {
            // A straight body's trailing back jump was trimmed at build, so
            // the end of the slice is the next iteration.
            return if plan.straight {
                BodyOut::Next
            } else {
                BodyOut::Fail
            };
        };
        let effect = match op {
            LOp::VecPush { vec, val } => match s_value(regs[usize::from(*val)]) {
                Some(v) => {
                    fx.vecs[usize::from(*vec)].push(v);
                    true
                }
                None => false,
            },
            LOp::MapGetOr {
                dst,
                map,
                key,
                default,
            } => map_get_or(regs, fx, *dst, *map, *key, *default),
            LOp::MapGetOpt { dst, map, key } => map_get_opt(regs, fx, *dst, *map, *key),
            LOp::MapHas { dst, map, key } => map_has(regs, fx, *dst, *map, *key),
            LOp::MapInsert { dst, map, key, val } => map_insert(regs, fx, *dst, *map, *key, *val),
            LOp::ItemIndex { dst, item, key } => item_index(regs, fx, items, *dst, *item, *key),
            other => {
                match eval_op(other, regs) {
                    OpOut::Fall => ip += 1,
                    OpOut::Fail => return BodyOut::Fail,
                    OpOut::Jump(LTo::Next) => return BodyOut::Next,
                    OpOut::Jump(LTo::Exit) => return BodyOut::Exit,
                    OpOut::Jump(LTo::Op(t)) => {
                        let t = t as usize;
                        if t <= ip {
                            steps += 1;
                            if steps > MAX_BODY_STEPS {
                                return BodyOut::Fail;
                            }
                        }
                        ip = t;
                    }
                }
                continue;
            }
        };
        if !effect {
            return BodyOut::Fail;
        }
        ip += 1;
    }
}

/// One iteration of an effects chunk: snapshot the registers, record each
/// vec's entry length, clear the map journal, run the body, and on failure
/// restore all three, so the failing item stays unconsumed for the generic
/// loop with its exact entry state.
fn effect_iteration(
    plan: &LoopPlan,
    regs: &mut [SVal],
    snapshot: &mut Vec<SVal>,
    fx: &mut Effects<'_, '_>,
    item: SVal,
    items: Option<&[Value]>,
) -> BodyOut {
    snapshot.clear();
    snapshot.extend_from_slice(regs);
    let mut lens = [0usize; MAX_PUSH_VECS];
    for (len, guard) in lens.iter_mut().zip(fx.vecs.iter()) {
        *len = guard.len();
    }
    fx.journal.clear();
    let body = run_body_effects(plan, regs, item, fx, items);
    if matches!(body, BodyOut::Fail) {
        regs.copy_from_slice(snapshot);
        for (guard, len) in fx.vecs.iter_mut().zip(lens) {
            guard.truncate(len);
        }
        unwind_maps(fx);
    }
    body
}

/// A chunk of range items through an effects plan, each iteration journaled
/// by `effect_iteration`.
fn range_effects_chunk(
    plan: &LoopPlan,
    regs: &mut [SVal],
    snapshot: &mut Vec<SVal>,
    fx: &mut Effects<'_, '_>,
    next: &mut i64,
    end: i64,
    inclusive: bool,
) -> ChunkOut {
    let mut advanced = 0i64;
    let out = |advanced, state| ChunkOut { advanced, state };
    for _ in 0..CHUNK {
        let done = if inclusive { *next > end } else { *next >= end };
        if done {
            return out(advanced, ChunkState::Done);
        }
        let item = *next;
        match effect_iteration(plan, regs, snapshot, fx, SVal::Int(item), None) {
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
            BodyOut::Fail => return out(advanced, ChunkState::Failed),
        }
    }
    out(advanced, ChunkState::More)
}

/// A chunk of `split_whitespace` words through an effects plan. A failing
/// iteration rewinds the offset to before its word, so the generic loop
/// re-pulls it through the same shared span walk.
fn words_effects_chunk(
    plan: &LoopPlan,
    regs: &mut [SVal],
    snapshot: &mut Vec<SVal>,
    fx: &mut Effects<'_, '_>,
    source: &RsStr,
    offset: &mut usize,
) -> ChunkOut {
    let mut advanced = 0i64;
    let out = |advanced, state| ChunkOut { advanced, state };
    for _ in 0..CHUNK {
        let before = *offset;
        let Some((start, end)) = next_word_span(source, offset) else {
            return out(advanced, ChunkState::Done);
        };
        let (Ok(start), Ok(end)) = (u32::try_from(start), u32::try_from(end)) else {
            *offset = before;
            return out(advanced, ChunkState::Failed);
        };
        let item = SVal::StrSpan { start, end };
        match effect_iteration(plan, regs, snapshot, fx, item, None) {
            BodyOut::Next => advanced += 1,
            BodyOut::Exit => {
                advanced += 1;
                return out(advanced, ChunkState::Exited);
            }
            BodyOut::Fail => {
                *offset = before;
                return out(advanced, ChunkState::Failed);
            }
        }
    }
    out(advanced, ChunkState::More)
}

/// A chunk of `find_iter` matches through an effects plan, the same rewind
/// contract as `words_effects_chunk`.
fn regex_effects_chunk(
    plan: &LoopPlan,
    regs: &mut [SVal],
    snapshot: &mut Vec<SVal>,
    fx: &mut Effects<'_, '_>,
    regex: &RegexValue,
    source: &RsStr,
    offset: &mut usize,
) -> ChunkOut {
    let mut advanced = 0i64;
    let out = |advanced, state| ChunkOut { advanced, state };
    for _ in 0..CHUNK {
        let before = *offset;
        let Some((start, end)) = regex_find_span(regex, source, offset) else {
            return out(advanced, ChunkState::Done);
        };
        let (Ok(start), Ok(end)) = (u32::try_from(start), u32::try_from(end)) else {
            *offset = before;
            return out(advanced, ChunkState::Failed);
        };
        let item = SVal::Span { start, end };
        match effect_iteration(plan, regs, snapshot, fx, item, None) {
            BodyOut::Next => advanced += 1,
            BodyOut::Exit => {
                advanced += 1;
                return out(advanced, ChunkState::Exited);
            }
            BodyOut::Fail => {
                *offset = before;
                return out(advanced, ChunkState::Failed);
            }
        }
    }
    out(advanced, ChunkState::More)
}

/// A chunk over a vec's elements through an effects plan, each item probed
/// in place through its `Item` slot, the json shape. A failing iteration
/// leaves the index on its item, so the generic loop re-binds it.
fn items_effects_chunk(
    plan: &LoopPlan,
    regs: &mut [SVal],
    snapshot: &mut Vec<SVal>,
    fx: &mut Effects<'_, '_>,
    items: &[Value],
    index: &mut usize,
) -> ChunkOut {
    let mut advanced = 0i64;
    let out = |advanced, state| ChunkOut { advanced, state };
    for _ in 0..CHUNK {
        if *index >= items.len() {
            return out(advanced, ChunkState::Done);
        }
        let Ok(idx) = u32::try_from(*index) else {
            return out(advanced, ChunkState::Failed);
        };
        match effect_iteration(plan, regs, snapshot, fx, SVal::Item(idx), Some(items)) {
            BodyOut::Next => {
                *index += 1;
                advanced += 1;
            }
            BodyOut::Exit => {
                *index += 1;
                advanced += 1;
                return out(advanced, ChunkState::Exited);
            }
            BodyOut::Fail => return out(advanced, ChunkState::Failed),
        }
    }
    out(advanced, ChunkState::More)
}

/// Zero-progress effects-runner failures before the plan is dropped from
/// the cache, so a loop whose entry state never runs as an effects plan
/// stops paying the setup per iteration.
const MAX_ZERO_FAILS: u32 = 32;

/// Count one zero-progress failure, and past the budget drop the plan from
/// the cache for good.
fn note_effects_fail(ctx: &StepCtx, plan: &LoopPlan, head: usize) {
    if plan.fails.fetch_add(1, Ordering::Relaxed) + 1 >= MAX_ZERO_FAILS {
        ctx.cur.loop_plans.lock().insert(head, None);
    }
}

/// Resolve the effects runner's tables against the frame: check the source
/// is one the chunks can walk, split each written base from sharing, the
/// split the generic path's per-iteration `UniqueReg` amounts to, and take
/// the storage handles. `None` fails the run over to the generic path: an
/// unsupported source, a base of the wrong shape, or two bases sharing one
/// storage, whose lock the chunk cannot take twice. The source check comes
/// first, so an unsupported source costs no setup. A span source's string
/// is immutable shared storage, so the snapshot taken here is the same
/// string every chunk walks.
fn effects_setup(
    ctx: &mut StepCtx,
    plan: &LoopPlan,
    handle: &Handle,
) -> Option<(Vec<List>, Vec<Map>, Option<RsStr>)> {
    let span_source: Option<RsStr> = match &*handle.lock() {
        Native::Iterator(
            IteratorState::Range { .. }
            | IteratorState::Values { .. }
            | IteratorState::Owned { .. },
        ) => None,
        Native::Iterator(
            IteratorState::SplitWhitespace { source, .. } | IteratorState::RegexFind { source, .. },
        ) => Some(source.clone()),
        _ => return None,
    };
    let mut lists: Vec<List> = Vec::with_capacity(plan.vecs.len());
    for &reg in &plan.vecs {
        ctx.stack[ctx.base + usize::from(reg)].make_unique();
        let Value::Vec(list) = ctx.get(reg) else {
            return None;
        };
        lists.push(list.clone());
    }
    let mut stores: Vec<Map> = Vec::with_capacity(plan.maps.len());
    for (&reg, &written) in plan.maps.iter().zip(&plan.maps_written) {
        if written {
            ctx.stack[ctx.base + usize::from(reg)].make_unique();
        }
        let Value::Map(store, MapKind::Map) = ctx.get(reg) else {
            return None;
        };
        stores.push(store.clone());
    }
    let aliased = (1..lists.len()).any(|i| lists[..i].iter().any(|h| Arc::ptr_eq(h, &lists[i])))
        || (1..stores.len()).any(|i| stores[..i].iter().any(|h| Arc::ptr_eq(h, &stores[i])));
    if aliased {
        return None;
    }
    Some((lists, stores, span_source))
}

/// Run a plan whose body pushes into vecs or probes maps, over a range,
/// `split_whitespace`, or `find_iter` source. Each pushed base and each
/// inserted map splits from sharing once at entry, the split the generic
/// path's per-iteration `UniqueReg` amounts to, and their storage stays
/// locked for the chunk. The locks drop around every Ctrl-C poll.
fn run_effects(
    ctx: &mut StepCtx,
    plan: &LoopPlan,
    head: usize,
    iter: u16,
    idx: u16,
    to: u32,
) -> Result<Option<Flow>> {
    let Value::Native(handle) = ctx.get(iter) else {
        return Ok(None);
    };
    let handle = handle.clone();
    let Some((lists, stores, span_source)) = effects_setup(ctx, plan, &handle) else {
        note_effects_fail(ctx, plan, head);
        return Ok(None);
    };
    let mut regs: Vec<SVal> = plan.regs.iter().map(|&r| SVal::of(ctx.get(r))).collect();
    let mut snapshot: Vec<SVal> = Vec::with_capacity(regs.len());
    let mut journal: Vec<MapUndo> = Vec::new();
    let mut consumed = 0i64;
    loop {
        let out = {
            let mut native = handle.lock();
            let mut vec_guards: Vec<_> = lists.iter().map(|l| l.lock()).collect();
            let mut map_guards: Vec<_> = stores.iter().map(|m| m.lock()).collect();
            let mut fx = Effects {
                vecs: &mut vec_guards,
                maps: &mut map_guards,
                stores: &stores,
                journal: &mut journal,
                source: span_source.as_ref(),
                strs: &plan.strs,
            };
            effects_chunk(plan, &mut regs, &mut snapshot, &mut fx, &mut native, &lists)
        };
        consumed += out.advanced;
        match out.state {
            ChunkState::NotSimple if consumed == 0 => {
                note_effects_fail(ctx, plan, head);
                return Ok(None);
            }
            ChunkState::NotSimple | ChunkState::Failed => {
                if consumed == 0 {
                    note_effects_fail(ctx, plan, head);
                }
                put_items(ctx, plan, &regs, &handle);
                write_back(ctx, plan, &regs, idx, consumed, span_source.as_ref());
                return Ok(None);
            }
            ChunkState::Done | ChunkState::Exited => {
                put_items(ctx, plan, &regs, &handle);
                write_back(ctx, plan, &regs, idx, consumed, span_source.as_ref());
                return Ok(Some(Flow::Jump(to as usize)));
            }
            ChunkState::More => {
                put_items(ctx, plan, &regs, &handle);
                write_back(ctx, plan, &regs, idx, consumed, span_source.as_ref());
                ctx.vm.run_pending_ctrlc()?;
            }
        }
    }
}

/// One locked chunk of the effects runner: dispatch on the source and walk
/// it through the matching chunk function.
fn effects_chunk(
    plan: &LoopPlan,
    regs: &mut [SVal],
    snapshot: &mut Vec<SVal>,
    fx: &mut Effects<'_, '_>,
    native: &mut Native,
    lists: &[List],
) -> ChunkOut {
    match native {
        Native::Iterator(IteratorState::Range {
            next,
            end,
            inclusive,
        }) => range_effects_chunk(plan, regs, snapshot, fx, next, *end, *inclusive),
        Native::Iterator(IteratorState::SplitWhitespace { source, offset }) => {
            let source = source.clone();
            words_effects_chunk(plan, regs, snapshot, fx, &source, offset)
        }
        Native::Iterator(IteratorState::RegexFind {
            regex,
            source,
            offset,
        }) => {
            let (regex, source) = (regex.clone(), source.clone());
            regex_effects_chunk(plan, regs, snapshot, fx, &regex, &source, offset)
        }
        Native::Iterator(IteratorState::Owned { values, index }) => {
            let items: &[Value] = values;
            items_effects_chunk(plan, regs, snapshot, fx, items, index)
        }
        // A source list aliasing a pushed base would deadlock the second
        // lock, and the generic path handles that fine.
        Native::Iterator(IteratorState::Values { values, index }) => {
            if lists.iter().any(|l| Arc::ptr_eq(l, values)) {
                ChunkOut {
                    advanced: 0,
                    state: ChunkState::NotSimple,
                }
            } else {
                let values = values.clone();
                let items = values.lock();
                items_effects_chunk(plan, regs, snapshot, fx, &items, index)
            }
        }
        _ => ChunkOut {
            advanced: 0,
            state: ChunkState::NotSimple,
        },
    }
}

/// Land every `Item` slot's boxed value in its frame register, the item
/// materialization twin of `write_back`'s span arm: `s_value` skips the
/// slot, so the register gets the exact value the generic `ForNext` would
/// have bound, read back from the source by position.
fn put_items(ctx: &mut StepCtx, plan: &LoopPlan, regs: &[SVal], handle: &Handle) {
    for (slot, sval) in regs.iter().enumerate() {
        let SVal::Item(idx) = *sval else {
            continue;
        };
        let idx = usize::try_from(idx).expect("u32 fits usize");
        let item = match &*handle.lock() {
            Native::Iterator(IteratorState::Values { values, .. }) => {
                values.lock().get(idx).cloned()
            }
            Native::Iterator(IteratorState::Owned { values, .. }) => values.get(idx).cloned(),
            _ => None,
        };
        if let Some(v) = item {
            ctx.put(plan.regs[slot], v);
        }
    }
}
