//! Scalar function plans: a self-recursive function whose whole body
//! compiles to integer, float, bool, and user-enum bytecode runs unboxed
//! inside one `CallFn` dispatch. The generic path pays a frame push, boxed
//! `Value` registers, and one dispatch per op for every call, which is the
//! whole cost of a call tree like `fib`, or of building and folding a
//! recursive enum like the binary-trees shape. The plan runs the tree on a
//! flat stack of unboxed values, and the enum values it builds or matches
//! live in one run-local boxed table the `Boxed` slots index.
//!
//! The subset makes such a body pure: no globals, no cells, no upvalues,
//! no vec or field access, and the only calls are recursion into the same
//! plan and the whitelisted numeric methods. Enum ops keep that purity, a
//! `NewEnum` builds a fresh value the way the generic `make_enum` does and
//! a `TestVariant` only reads, so failure recovery needs no journal and no
//! replay. Any runtime surprise, an overflow, an unsupported width, an
//! argument the slots cannot hold, discards the run and the generic path
//! runs the whole call from scratch with identical semantics, panic op and
//! line included.
//!
//! The plan IR, its translation, and the op evaluator live in
//! `scalar_loop`, the runner here holds the frame stack the `CallSelf` and
//! `Ret` ops need and the boxed table the enum ops need.

use std::mem::take;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::Result;
use parking_lot::Mutex;

use super::bytecode::{Chunk, Op, PPat, PathId};
use super::scalar_fold::fold_moves;
use super::scalar_loop::{
    LOp, LTo, MAX_CALL_ARGS, MAX_ENUM_ARGS, NO_SLOT, OpOut, Region, eval_op, slot, translate,
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
    /// Whether the body has enum ops, the gate for boxing an enum argument
    /// instead of failing the call over.
    uses_boxed: bool,
    /// Consecutive failed runs, cleared by a success, see `MAX_FAILS`.
    fails: AtomicU32,
}

/// The `Op::MakeEnum` and `Op::LoadEnum` arms of `build`: the variant's
/// names and the payload slots into a `NewEnum`, or one prebuilt value
/// into a `UnitEnum` for a payload-free variant.
fn new_enum(
    chunk: &Chunk,
    regs: &mut Vec<u16>,
    dst: u16,
    info: u16,
    base: u16,
    count: u16,
) -> Option<LOp> {
    if usize::from(count) > MAX_ENUM_ARGS {
        return None;
    }
    let variant = &chunk.enum_variants[info as usize];
    if count == 0 {
        return Some(LOp::UnitEnum {
            dst: slot(regs, dst)?,
            value: Value::Enum {
                def: variant.def.clone(),
                variant: variant.variant,
                data: Arc::new(Mutex::new(Vec::new())),
            },
        });
    }
    let mut args = [0u16; MAX_ENUM_ARGS];
    for (arg, reg) in args.iter_mut().zip(base..base.saturating_add(count)) {
        *arg = slot(regs, reg)?;
    }
    Some(LOp::NewEnum {
        dst: slot(regs, dst)?,
        def: variant.def.clone(),
        variant: variant.variant,
        args,
        argc: u8::try_from(count).ok()?,
    })
}

/// The `Op::CallPath` arm of `build` for `Box::new(x)`, whose bridge is the
/// identity, so the plan op is a move.
fn box_new(
    chunk: &Chunk,
    regs: &mut Vec<u16>,
    dst: u16,
    path: u16,
    base: u16,
    argc: u16,
) -> Option<LOp> {
    if argc != 1 || dst == u16::MAX {
        return None;
    }
    let path = &chunk.paths[path as usize];
    if path.id != PathId::BoxNew || path.coerce.is_some() {
        return None;
    }
    Some(LOp::Move {
        dst: slot(regs, dst)?,
        src: slot(regs, base)?,
    })
}

/// The enum fallback of the `Op::TestBind` arm of `build`: a unit variant
/// path pattern, or a tuple variant pattern whose elements are all plain
/// bindings, into a `TestVariant`. Empty binds mean the path pattern, which
/// tests the variant name alone, and non-empty binds carry the tuple
/// pattern's arity, which the payload must match, both mirroring the enum
/// arms of the generic `try_bind`.
fn test_variant(chunk: &Chunk, regs: &mut Vec<u16>, val: u16, pat: u16, dst: u16) -> Option<LOp> {
    let info = &chunk.pats[pat as usize];
    let (tag, binds) = match &info.pat {
        // The `Null` name also matches an `Option::None` value on the
        // generic path, a json rule the plan does not mirror.
        PPat::Path { tag } if tag.name.is_some() && !tag.is_named("Null") => (tag, Vec::new()),
        PPat::TupleStruct { tag, elems } if tag.name.is_some() && !elems.is_empty() => {
            let mut binds = Vec::with_capacity(elems.len());
            for elem in elems {
                let PPat::Ident {
                    name: elem_name,
                    sub: None,
                } = elem
                else {
                    return None;
                };
                let (_, reg) = info.binds.iter().find(|(n, _)| n == elem_name)?;
                binds.push(slot(regs, *reg)?);
            }
            (tag, binds)
        }
        _ => return None,
    };
    Some(LOp::TestVariant {
        dst: slot(regs, dst)?,
        val: slot(regs, val)?,
        tag: tag.clone(),
        binds: binds.into_boxed_slice(),
    })
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
            Op::MakeEnum {
                dst,
                info,
                base,
                count,
            } => new_enum(chunk, &mut regs, *dst, *info, *base, *count)?,
            Op::LoadEnum { dst, info } => new_enum(chunk, &mut regs, *dst, *info, 0, 0)?,
            Op::CallPath {
                dst,
                path,
                base,
                argc,
            } => match box_new(chunk, &mut regs, *dst, *path, *base, *argc) {
                Some(lop) => lop,
                None => translate(vm, chunk, &region, &mut regs, None, &mut try_mask, op)?,
            },
            Op::TestBind { val, pat, dst } => {
                match translate(vm, chunk, &region, &mut regs, None, &mut try_mask, op) {
                    Some(lop) => lop,
                    None => test_variant(chunk, &mut regs, *val, *pat, *dst)?,
                }
            }
            other => translate(vm, chunk, &region, &mut regs, None, &mut try_mask, other)?,
        };
        ops.push(lop);
    }
    if !recursive {
        return None;
    }
    fold_moves(&mut ops, NO_SLOT, &chunk_reads(chunk), &regs);
    let uses_boxed = ops.iter().any(|op| {
        matches!(
            op,
            LOp::NewEnum { .. } | LOp::UnitEnum { .. } | LOp::TestVariant { .. }
        )
    });
    Some(FnPlan {
        ops,
        num_slots: regs.len(),
        num_params: chunk.num_params,
        uses_boxed,
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

/// A slot's boxed value for an enum payload: the boxed table entry it
/// names, taken out, or the value of a scalar slot. `None` fails the run
/// over. Taking is safe because a payload argument is moved in real Rust,
/// so a program that passed the checker never reads the consumed slot
/// again, the same single-use invariant the register allocator's
/// no-reuse rule rests on.
fn payload_value(v: SVal, boxed: &mut [Value]) -> Option<Value> {
    match v {
        SVal::Boxed(i) => Some(take(&mut boxed[i as usize])),
        other => s_value(other),
    }
}

/// The enum arms of the runner's loop: build a value into the boxed table,
/// or test one against a variant pattern. `None` fails the run over to the
/// generic path.
fn enum_op(op: &LOp, regs: &mut [SVal], boxed: &mut Vec<Value>) -> Option<()> {
    match op {
        LOp::UnitEnum { dst, value } => {
            let idx = u32::try_from(boxed.len()).ok()?;
            boxed.push(value.clone());
            regs[usize::from(*dst)] = SVal::Boxed(idx);
        }
        LOp::NewEnum {
            dst,
            def,
            variant,
            args,
            argc,
        } => {
            let mut data = Vec::with_capacity(usize::from(*argc));
            for arg in &args[..usize::from(*argc)] {
                data.push(payload_value(regs[usize::from(*arg)], boxed)?);
            }
            let idx = u32::try_from(boxed.len()).ok()?;
            boxed.push(Value::Enum {
                def: def.clone(),
                variant: *variant,
                data: Arc::new(Mutex::new(data)),
            });
            regs[usize::from(*dst)] = SVal::Boxed(idx);
        }
        LOp::TestVariant {
            dst,
            val,
            tag,
            binds,
        } => {
            let SVal::Boxed(i) = regs[usize::from(*val)] else {
                return None;
            };
            // Anything but an enum keeps its own generic rules, the
            // pre-unwrapped Some and the json shape matches, so it fails
            // the run over.
            let (mut matched, payload) = {
                let Value::Enum { def, variant, data } = &boxed[i as usize] else {
                    return None;
                };
                let matched = tag.matches(def, *variant);
                // The list handle alone, so the elements bind straight out
                // of the locked storage with no payload copy, the clone
                // the generic bind pays.
                let payload = (matched && !binds.is_empty()).then(|| data.clone());
                (matched, payload)
            };
            if let Some(list) = payload {
                let items = list.lock();
                if items.len() == binds.len() {
                    for (bind, v) in binds.iter().zip(items.iter()) {
                        let scalar = SVal::of(v);
                        regs[usize::from(*bind)] = if matches!(scalar, SVal::Opaque) {
                            let idx = u32::try_from(boxed.len()).ok()?;
                            boxed.push(v.clone());
                            SVal::Boxed(idx)
                        } else {
                            scalar
                        };
                    }
                } else {
                    // A payload the pattern's arity cannot bind answers
                    // false with nothing bound, mirroring `bind_seq`.
                    matched = false;
                }
            }
            regs[usize::from(*dst)] = SVal::Bool(matched);
        }
        _ => unreachable!("only enum ops reach enum_op"),
    }
    Some(())
}

/// Run one call tree, `None` when an op fails and the generic path should
/// run the whole call instead. `depth_budget` is the frame count left under
/// the generic `MAX_CALL_DEPTH`, so the plan fails over exactly where the
/// generic path would report call depth exceeded. `boxed` arrives holding
/// the call's enum arguments and grows with every value the run builds or
/// binds.
///
/// Kept out of line on purpose. Inlined through `try_call` into `step` it
/// drags the plan loop into the dispatch frame of every op, which cost
/// `binary_trees` 14 percent.
#[inline(never)]
fn run(
    vm: &Arc<Vm>,
    plan: &FnPlan,
    args: &[SVal],
    depth_budget: usize,
    mut boxed: Vec<Value>,
) -> Result<Option<Value>> {
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
                let callee = base + slots;
                // Frames reuse the high-water stack, stale slots included:
                // the compiler writes every register before its first
                // read, so a leftover value below the mark is never
                // observable.
                if stack.len() < callee + slots {
                    stack.resize(callee + slots, SVal::Unit);
                }
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
            Some(op @ (LOp::UnitEnum { .. } | LOp::NewEnum { .. } | LOp::TestVariant { .. })) => {
                if enum_op(op, &mut stack[base..base + slots], &mut boxed).is_none() {
                    return Ok(None);
                }
                ip += 1;
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
                return Ok(match v {
                    SVal::Boxed(i) => Some(take(&mut boxed[i as usize])),
                    other => s_value(other),
                });
            };
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
    let mut boxed: Vec<Value> = Vec::new();
    for (val, reg) in vals.iter_mut().zip(abase..abase.saturating_add(argc)) {
        *val = SVal::of(ctx.get(reg));
        if matches!(*val, SVal::Opaque) {
            let arg = ctx.get(reg);
            let boxable = plan.uses_boxed && matches!(arg, Value::Enum { .. });
            let idx = u32::try_from(boxed.len()).ok().filter(|_| boxable);
            let Some(idx) = idx else {
                note_fail(&plan, callee);
                return Ok(None);
            };
            boxed.push(arg.clone());
            *val = SVal::Boxed(idx);
        }
    }
    // The intercepted call itself would be one generic frame, and each plan
    // frame one more, so the budget maps plan depth onto the exact frame
    // count the generic loop caps.
    let budget = MAX_CALL_DEPTH - ctx.depth - 1;
    if let Some(v) = run(ctx.vm, &plan, &vals[..usize::from(argc)], budget, boxed)? {
        plan.fails.store(0, Ordering::Relaxed);
        Ok(Some(v))
    } else {
        note_fail(&plan, callee);
        Ok(None)
    }
}
