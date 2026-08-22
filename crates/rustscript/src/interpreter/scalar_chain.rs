//! `sum`, `count`, `any` and `all` over `map` and `filter` stages run unboxed inside 1 dispatch.
//! Every stage is pure, so nothing advances the source until the whole reduction succeeds, and any
//! surprise discards the run for the generic path to re-run. The plan IR lives in `scalar_loop`.

use std::sync::Arc;

use anyhow::Result;
use parking_lot::Mutex;

use super::bytecode::{Op, ScalarTy};
use super::iterator::IteratorState;
use super::native::Native;
use super::scalar_fold::fold_moves;
use super::scalar_loop::{LOp, LTo, MAX_SLOTS, NO_SLOT, OpOut, Region, eval_op, translate};
use super::scalar_reads::chunk_reads;
use super::scalar_val::{SVal, truthy};
use super::shared::usize_value;
use super::value::{ClosureData, Upvalue, Value};
use super::vm::Vm;

type Handle = Arc<Mutex<Native>>;

/// source elements between Ctrl-C polls, the source lock drops around each
const CHUNK: usize = 4096;

/// So a body that runs long fails over to the generic path, which polls Ctrl-C.
const MAX_BODY_STEPS: u32 = 65_536;

/// `Any` and `All` carry their predicate, applied after the stages.
pub(super) enum ChainReduce<'a> {
    /// with the turbofish target
    Sum(Option<&'a ScalarTy>),
    Count,
    Any(&'a Arc<ClosureData>),
    All(&'a Arc<ClosureData>),
}

/// Slot 0 is the parameter, `Ret` returns from the call.
struct ChainPlan {
    ops: Vec<LOp>,
    slots: Vec<SVal>,
}

enum Stage {
    Map(ChainPlan),
    Filter(ChainPlan),
}

/// Snapshotted at analysis, the run consumes nothing until it commits.
enum Base {
    /// the index a `Values` or `Owned` state would read next
    Indexed { start: usize },
    Range {
        next: i64,
        end: i64,
        inclusive: bool,
    },
}

/// None when any op, capture or shape falls outside the subset.
fn closure_plan(vm: &Vm, clo: &ClosureData) -> Option<ChainPlan> {
    let chunk = &clo.chunk;
    if chunk.path_forwarder || !chunk.generics.is_empty() || chunk.num_params != 1 {
        return None;
    }
    let mut regs: Vec<u16> = vec![0];
    // a closure body has no loop to re-enter, and falling off the end returns unit like the
    // generic frame loop
    let region = Region {
        head: usize::MAX,
        body: 0,
        exit: chunk.code.len(),
    };
    let mut try_mask = 0u64;
    let mut ops = Vec::with_capacity(chunk.code.len());
    for op in &chunk.code {
        let lop = match op {
            Op::Ret { src } => LOp::Ret {
                src: slot_of(&mut regs, *src)?,
            },
            // a mutable cell could change between calls, so only an immutable scalar capture is a
            // safe constant
            Op::LoadUpvalue { dst, idx } => {
                let dst = slot_of(&mut regs, *dst)?;
                match clo.captured.get(*idx as usize)? {
                    Upvalue::Value(Value::Int(v)) => LOp::LoadInt { dst, v: *v },
                    Upvalue::Value(Value::Float(v)) => LOp::LoadFloat { dst, v: *v },
                    Upvalue::Value(Value::Bool(v)) => LOp::LoadBool { dst, v: *v },
                    _ => return None,
                }
            }
            other => translate(vm, chunk, &region, &mut regs, None, &mut try_mask, other)?,
        };
        ops.push(lop);
    }
    fold_moves(&mut ops, NO_SLOT, &chunk_reads(chunk), &regs);
    Some(ChainPlan {
        ops,
        slots: vec![SVal::Opaque; regs.len()],
    })
}

fn slot_of(regs: &mut Vec<u16>, r: u16) -> Option<u16> {
    if let Some(i) = regs.iter().position(|&x| x == r) {
        return u16::try_from(i).ok();
    }
    if regs.len() >= MAX_SLOTS {
        return None;
    }
    regs.push(r);
    u16::try_from(regs.len() - 1).ok()
}

impl ChainPlan {
    /// `None` fails the whole reduction over.
    fn eval(&mut self, arg: SVal) -> Option<SVal> {
        // slots reset per call, a leftover would be a value the generic call never sees
        self.slots.fill(SVal::Opaque);
        self.slots[0] = arg;
        let mut ip = 0usize;
        let mut steps = 0u32;
        loop {
            let Some(op) = self.ops.get(ip) else {
                return Some(SVal::Unit);
            };
            if let LOp::Ret { src } = op {
                return Some(self.slots[usize::from(*src)]);
            }
            match eval_op(op, &mut self.slots) {
                OpOut::Fall => ip += 1,
                OpOut::Fail | OpOut::Jump(LTo::Next) => return None,
                OpOut::Jump(LTo::Exit) => return Some(SVal::Unit),
                OpOut::Jump(LTo::Op(t)) => {
                    let t = t as usize;
                    // only backward jumps count, so straight bodies pay no counter
                    if t <= ip {
                        steps += 1;
                        if steps > MAX_BODY_STEPS {
                            return None;
                        }
                    }
                    ip = t;
                }
            }
        }
    }
}

/// Walk the chain down to a supported source, translating every closure. The source handle comes
/// back separately, the run reads through it and the commit advances it.
fn analyze(vm: &Vm, handle: &Handle) -> Option<(Vec<Stage>, Base, Handle)> {
    let mut stages: Vec<Stage> = Vec::new();
    let mut cur = handle.clone();
    loop {
        let next = {
            let native = cur.lock();
            let Native::Iterator(state) = &*native else {
                return None;
            };
            match state {
                IteratorState::Map { source, closure } => {
                    stages.push(Stage::Map(closure_plan(vm, closure)?));
                    source.clone()
                }
                IteratorState::Filter { source, closure } => {
                    stages.push(Stage::Filter(closure_plan(vm, closure)?));
                    source.clone()
                }
                IteratorState::Values { index, .. } | IteratorState::Owned { index, .. } => {
                    let base = Base::Indexed { start: *index };
                    drop(native);
                    stages.reverse();
                    return Some((stages, base, cur.clone()));
                }
                IteratorState::Range {
                    next,
                    end,
                    inclusive,
                } => {
                    let base = Base::Range {
                        next: *next,
                        end: *end,
                        inclusive: *inclusive,
                    };
                    drop(native);
                    stages.reverse();
                    return Some((stages, base, cur.clone()));
                }
                _ => return None,
            }
        };
        cur = next;
    }
}

/// Mirrors the accumulator of the generic drain.
enum Acc {
    /// The i128 accumulator and bounds of `sum_values`. Without a target the first tagged element
    /// sets the bounds.
    Sum {
        total: i128,
        low: i128,
        high: i128,
        bounded: bool,
    },
    Count(usize),
    Any(bool),
    All(bool),
}

impl Acc {
    /// None when the reduction falls outside the subset. A float sum gives a float even when empty.
    fn new(reduce: &ChainReduce) -> Option<Acc> {
        Some(match reduce {
            ChainReduce::Sum(target) => {
                let (low, high) = match target {
                    Some(ScalarTy::Int(width)) => (width.min(), width.max()),
                    Some(_) => return None,
                    None => (i128::from(i64::MIN), i128::from(i64::MAX)),
                };
                Acc::Sum {
                    total: 0,
                    low,
                    high,
                    bounded: target.is_some(),
                }
            }
            ChainReduce::Count => Acc::Count(0),
            ChainReduce::Any(_) => Acc::Any(false),
            ChainReduce::All(_) => Acc::All(true),
        })
    }

    /// `None` fails the reduction over, `Some(true)` exits early like the generic `any` and `all`.
    fn feed(&mut self, v: SVal) -> Option<bool> {
        match self {
            Acc::Sum {
                total,
                low,
                high,
                bounded,
            } => {
                let n = match v {
                    SVal::Int(i) => i128::from(i),
                    SVal::IntW(s, w) => {
                        if !*bounded {
                            (*low, *high) = (w.min(), w.max());
                            *bounded = true;
                        }
                        w.decode(s)
                    }
                    _ => return None,
                };
                // mirrors `sum_values`, so an overflow falls back and the generic re-run raises
                // the exact error
                *total = total.checked_add(n)?;
                if *total < *low || *total > *high {
                    return None;
                }
            }
            Acc::Count(count) => *count += 1,
            Acc::Any(found) => {
                if truthy(v) {
                    *found = true;
                    return Some(true);
                }
            }
            Acc::All(all) => {
                if !truthy(v) {
                    *all = false;
                    return Some(true);
                }
            }
        }
        Some(false)
    }

    fn finish(self, reduce: &ChainReduce) -> Value {
        match self {
            Acc::Sum { total, .. } => {
                if let ChainReduce::Sum(Some(ScalarTy::Int(width))) = reduce {
                    Value::int_of_width(total, *width)
                } else {
                    Value::Int(i64::try_from(total).expect("sum is range-checked per step"))
                }
            }
            Acc::Count(count) => usize_value(count),
            Acc::Any(found) => Value::Bool(found),
            Acc::All(all) => Value::Bool(all),
        }
    }
}

enum SpanOut {
    More,
    Done,
    Fail,
}

/// The local cursor commits nothing until the whole reduction succeeds.
struct ChainRun {
    stages: Vec<Stage>,
    predicate: Option<ChainPlan>,
    acc: Acc,
    /// the commit offset
    done: usize,
    /// live only for a `Range` base
    cursor: i64,
}

impl ChainRun {
    /// `None` fails over, `Some(true)` exits early.
    fn one(&mut self, item: SVal) -> Option<bool> {
        let mut v = item;
        for stage in &mut self.stages {
            match stage {
                Stage::Map(plan) => v = plan.eval(v)?,
                Stage::Filter(plan) => {
                    if !truthy(plan.eval(v)?) {
                        return Some(false);
                    }
                }
            }
        }
        if let Some(plan) = &mut self.predicate {
            v = plan.eval(v)?;
        }
        self.acc.feed(v)
    }

    /// A non scalar element fails over, nothing was committed.
    fn slice_span(&mut self, items: &[Value], start: usize) -> SpanOut {
        for _ in 0..CHUNK {
            let Some(item) = items.get(start + self.done) else {
                return SpanOut::Done;
            };
            let item = SVal::of(item);
            if matches!(item, SVal::Opaque) {
                return SpanOut::Fail;
            }
            self.done += 1;
            match self.one(item) {
                Some(false) => {}
                Some(true) => return SpanOut::Done,
                None => return SpanOut::Fail,
            }
        }
        SpanOut::More
    }

    /// Mirrors the generic `range_step`.
    fn range_span(&mut self, end: i64, inclusive: bool) -> SpanOut {
        for _ in 0..CHUNK {
            let done = if inclusive {
                self.cursor > end
            } else {
                self.cursor >= end
            };
            if done {
                return SpanOut::Done;
            }
            let item = self.cursor;
            self.cursor = self.cursor.wrapping_add(1);
            match self.one(SVal::Int(item)) {
                Some(false) => {}
                Some(true) => return SpanOut::Done,
                None => return SpanOut::Fail,
            }
        }
        SpanOut::More
    }
}

/// `None` means the generic path should drain the chain, with every iterator state untouched.
pub(super) fn try_reduce(
    vm: &Arc<Vm>,
    iterator: &Handle,
    reduce: &ChainReduce,
) -> Result<Option<Value>> {
    let Some(acc) = Acc::new(reduce) else {
        return Ok(None);
    };
    let Some((stages, base, source)) = analyze(vm, iterator) else {
        return Ok(None);
    };
    let predicate = match reduce {
        ChainReduce::Any(clo) | ChainReduce::All(clo) => {
            let Some(plan) = closure_plan(vm, clo) else {
                return Ok(None);
            };
            Some(plan)
        }
        _ => None,
    };
    let mut run = ChainRun {
        stages,
        predicate,
        acc,
        done: 0,
        cursor: match base {
            Base::Range { next, .. } => next,
            Base::Indexed { .. } => 0,
        },
    };
    loop {
        let out = match base {
            Base::Indexed { start } => {
                let native = source.lock();
                match &*native {
                    Native::Iterator(IteratorState::Values { values, .. }) => {
                        let items = values.lock();
                        run.slice_span(&items, start)
                    }
                    Native::Iterator(IteratorState::Owned { values, .. }) => {
                        run.slice_span(values, start)
                    }
                    _ => SpanOut::Fail,
                }
            }
            Base::Range { end, inclusive, .. } => run.range_span(end, inclusive),
        };
        match out {
            SpanOut::Fail => return Ok(None),
            SpanOut::Done => break,
            // the source lock is dropped so a Ctrl-C handler can't deadlock on it
            SpanOut::More => vm.run_pending_ctrlc()?,
        }
    }
    // commit the consumed elements, the state the generic pulls would leave
    {
        let mut native = source.lock();
        match &mut *native {
            Native::Iterator(
                IteratorState::Values { index, .. } | IteratorState::Owned { index, .. },
            ) => {
                if let Base::Indexed { start } = base {
                    *index = start + run.done;
                }
            }
            Native::Iterator(IteratorState::Range { next, .. }) => {
                *next = run.cursor;
            }
            _ => {}
        }
    }
    Ok(Some(run.acc.finish(reduce)))
}
