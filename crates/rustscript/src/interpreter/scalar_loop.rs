//! Scalar loop specialization for `for` bodies and backward-jump loops.
//!
//! A `for` loop over a string's bytes or an integer range pays the full VM
//! machinery per item: the iterator lock, a boxed `Value` per element, and
//! one dispatch per body op. A body that only moves plain integers, floats,
//! and booleans does not need any of that. This module translates such a body
//! once per loop into a small plan over unboxed scalar registers and runs
//! the whole loop inside one `ForNext` dispatch.
//!
//! A `while` or `loop` loop is the same story without an item source. Its
//! backward jump closes a region whose only ways out are the jump back to
//! the head and the jumps to the op after it, so the whole region runs as a
//! plan inside one `Jump` dispatch, condition included.
//!
//! The subset is strict on purpose, and every runtime surprise falls back to
//! the generic path with identical semantics. Register values are loaded at
//! loop entry, a value the plan cannot read is poison that aborts on first
//! read, and arithmetic runs through the same width-checked cores the
//! generic ops use. On any failure the registers are rebuilt to the state at
//! the start of the failing iteration, the source is left with that item
//! unconsumed, and the generic loop re-runs it, so an overflow panics on the
//! exact op and line the generic path panics on.
//!
//! This module holds the shared machinery: the plan IR, its translation
//! from bytecode, the move-folding cleanup, and the op evaluator. The
//! runners live in `scalar_for` and `scalar_while`.

use super::bytecode::{BinKind, Chunk, Const, Member, Op, UnKind};
use super::numeric::IntWidth;
use super::scalar_reads::chunk_reads;
use super::scalar_val::{
    SVal, s_bin, s_cast, s_cast_f64, s_cmp, s_f64_from, s_float_method, s_int_method, s_un,
    s_value, truthy,
};
use super::typeir::CastIr;
use super::vm::Vm;
use super::vm_step::StepCtx;

/// Register cap per plan, bounding the entry load and writeback cost.
const MAX_SLOTS: usize = 64;

/// The plan slot sentinel for a discarded result, and the `val_slot` of a
/// while plan, which has no item register. No real slot reaches it, the
/// slot cap is far lower.
pub(super) const NO_SLOT: u16 = u16::MAX;

/// A jump target inside the plan. `Next` is the loop head, one finished
/// iteration, and `Exit` is the op after the loop, a `break` or exhaustion.
#[derive(Clone, Copy)]
pub(super) enum LTo {
    Op(u32),
    Next,
    Exit,
}

/// One plan op. Registers are dense plan slots, not frame registers.
pub(super) enum LOp {
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
    /// An f64 literal, from a `LoadConst` whose constant is a `Const::Float`.
    LoadFloat {
        dst: u16,
        v: f64,
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
    /// An `as f64` cast.
    CastF64 {
        dst: u16,
        src: u16,
    },
    /// An unshadowed `f64::from(x)` call, whose saturating image conversion
    /// differs from the exact `as` cast, see `s_f64_from`.
    F64From {
        dst: u16,
        src: u16,
    },
    /// A whitelisted numeric method, `n.is_multiple_of(2)` or `f.sqrt()`.
    /// The receiver decides the table at run time, integers answer from
    /// `s_int_method` and floats from `s_float_method`, the same split the
    /// generic dispatch makes. `dst` is `NO_SLOT` when the compiler
    /// discarded the result.
    NumMethod {
        dst: u16,
        recv: u16,
        args: [u16; 2],
        argc: u8,
        name: Box<str>,
    },
    /// `dst = vec[idx]` on one of the plan's locked vecs. `vec` indexes the
    /// plan's vec table, not a slot. A non-scalar element or a bad index
    /// fails the iteration over to the generic path.
    VecGet {
        dst: u16,
        vec: u16,
        idx: u16,
    },
    /// `vec[idx] = val`, journaled so a failing iteration can undo it.
    VecSet {
        vec: u16,
        idx: u16,
        val: u16,
    },
    /// The element `Arc` of `vec[idx]` into the run's handle table, split
    /// from sharing first when the generic op was a `UniqueIndex`. A
    /// non-struct element or a bad index fails the iteration over to the
    /// generic path.
    ElemRef {
        handle: u16,
        vec: u16,
        idx: u16,
        unique: bool,
    },
    /// `dst = handle.member`, a scalar field of a held element.
    FieldGet {
        dst: u16,
        handle: u16,
        member: Member,
    },
    /// `handle.member = val`, journaled so a failing iteration can undo it.
    FieldSet {
        handle: u16,
        member: Member,
        val: u16,
    },
    /// The `SetIndex` writeback of a place chain: store the held element
    /// back into its vec slot, journaled like a vec write.
    ElemBack {
        vec: u16,
        idx: u16,
        handle: u16,
    },
    /// A `UniqueReg` on a vec base. The plan split the vec from sharing
    /// once at entry, so per-write splits inside the loop have nothing to
    /// do, but the op keeps its position for jump targets.
    Nop,
}

/// Integer methods a plan may run: pure, scalar in and out, and answered by
/// `int_method` for every integer receiver, so the plan call and the generic
/// call hit the same table. Names the table rejects at runtime, `abs` on an
/// unsigned width for one, fail the iteration over to the generic path.
fn scalar_int_method(name: &str) -> bool {
    matches!(
        name,
        "is_multiple_of"
            | "min"
            | "max"
            | "clamp"
            | "abs"
            | "signum"
            | "pow"
            | "isqrt"
            | "div_euclid"
            | "rem_euclid"
            | "saturating_add"
            | "saturating_sub"
            | "saturating_mul"
            | "wrapping_add"
            | "wrapping_sub"
            | "wrapping_mul"
            | "wrapping_neg"
            | "count_ones"
            | "count_zeros"
            | "leading_zeros"
            | "trailing_zeros"
            | "rotate_left"
            | "rotate_right"
            | "swap_bytes"
            | "reverse_bits"
    )
}

/// Float methods a plan may run on an f64 receiver: pure, scalar in and
/// out, and answered by `s_float_method`, which mirrors the float arms of
/// `shared::num_core`.
fn scalar_float_method(name: &str) -> bool {
    matches!(
        name,
        "sqrt"
            | "floor"
            | "ceil"
            | "round"
            | "trunc"
            | "fract"
            | "recip"
            | "powi"
            | "powf"
            | "mul_add"
            | "is_nan"
            | "is_finite"
            | "is_infinite"
            | "is_sign_positive"
            | "is_sign_negative"
    )
}

pub struct LoopPlan {
    pub(super) ops: Vec<LOp>,
    /// The frame register behind each plan slot.
    pub(super) regs: Vec<u16>,
    /// The slot of the `ForNext` value register, written per item.
    pub(super) val_slot: u16,
    /// True when the body is one basic block: no jump ops except the final
    /// `Jump` back to the head. Such a body runs as a plain slice walk with
    /// no instruction pointer.
    pub(super) straight: bool,
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

/// The chunk region one plan translates: the loop head, the first
/// translated op, and the exit one past the region. The `for` plan's body
/// starts one past its `ForNext` head, the while plan's at the head itself.
pub(super) struct Region {
    pub(super) head: usize,
    pub(super) body: usize,
    pub(super) exit: usize,
}

/// Map a chunk jump target into the plan whose translated ops start at
/// `region.body`.
fn target(region: &Region, t: u32) -> Option<LTo> {
    let t = t as usize;
    if t == region.head {
        Some(LTo::Next)
    } else if t == region.exit {
        Some(LTo::Exit)
    } else if t >= region.body && t < region.exit {
        u32::try_from(t - region.body).ok().map(LTo::Op)
    } else {
        None
    }
}

/// The vec context of a while plan: the region's vec base registers, and
/// the handle registers, each the `dst` of an index into a base whose value
/// the region reads or writes fields through.
pub(super) struct PlanVecs<'a> {
    pub(super) bases: &'a [u16],
    pub(super) handles: &'a [u16],
}

/// Translate one bytecode op, or answer None when it falls outside the
/// subset, which rejects the whole loop. `vecs` is the region's vec context
/// when the plan supports vec indexing, the while plan does, and `None` for
/// the `for` plan, which rejects vec ops.
pub(super) fn translate(
    vm: &Vm,
    chunk: &Chunk,
    region: &Region,
    regs: &mut Vec<u16>,
    vecs: Option<&PlanVecs>,
    op: &Op,
) -> Option<LOp> {
    if matches!(
        op,
        Op::Index { .. }
            | Op::UniqueIndex { .. }
            | Op::SetIndex { .. }
            | Op::UniqueReg { .. }
            | Op::GetField { .. }
            | Op::UniqueField { .. }
            | Op::SetField { .. }
    ) {
        return translate_vec(chunk, regs, vecs, op);
    }
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
        Op::LoadConst { dst, k } => match chunk.consts[*k as usize] {
            Const::Float(v) => LOp::LoadFloat {
                dst: slot(regs, *dst)?,
                v,
            },
            _ => return None,
        },
        Op::LoadBool { dst, v } => LOp::LoadBool {
            dst: slot(regs, *dst)?,
            v: *v,
        },
        // `*r` on a plain value copies it, the way the generic `deref`
        // answers a non-reference, so a deref is a move. A real reference
        // or cell loads as `Opaque`, so any use of the copy fails the
        // iteration over to the generic path, which derefs for real.
        Op::Move { dst, src } | Op::Deref { dst, src } => LOp::Move {
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
            to: target(region, *to)?,
        },
        Op::JumpIfFalse { cond, to } => LOp::JumpIfFalse {
            cond: slot(regs, *cond)?,
            to: target(region, *to)?,
        },
        Op::JumpIfTrue { cond, to } => LOp::JumpIfTrue {
            cond: slot(regs, *cond)?,
            to: target(region, *to)?,
        },
        Op::CmpJump { a, b, op, to } => LOp::CmpJump {
            a: slot(regs, *a)?,
            b: slot(regs, *b)?,
            op: *op,
            to: target(region, *to)?,
        },
        Op::CmpJumpImm { a, imm, op, to } => LOp::CmpJumpImm {
            a: slot(regs, *a)?,
            imm: *imm,
            op: *op,
            to: target(region, *to)?,
        },
        Op::Cast { dst, src, ty } => match chunk.casts[*ty as usize] {
            CastIr::Int(w) if !w.is_big() => LOp::Cast {
                dst: slot(regs, *dst)?,
                src: slot(regs, *src)?,
                w,
            },
            CastIr::F64 => LOp::CastF64 {
                dst: slot(regs, *dst)?,
                src: slot(regs, *src)?,
            },
            _ => return None,
        },
        Op::Method { .. } => return translate_method(chunk, regs, op),
        Op::CallPath { .. } => return translate_call(vm, chunk, regs, op),
        // A nested loop's entry hook has nothing to do inside a plan, which
        // already runs the nested loop unboxed, but keeps its position.
        Op::LoopHead { .. } => LOp::Nop,
        _ => return None,
    })
}

/// The `Op::CallPath` arm of `translate`: only a plain `f64::from(x)` call
/// maps, and only while nothing shadows the bridge arm `s_f64_from` mirrors.
/// A user function or user method named `f64::from` resolves first on the
/// generic path, and a coercion on the call site has no plan equivalent, so
/// either rejects the loop.
fn translate_call(vm: &Vm, chunk: &Chunk, regs: &mut Vec<u16>, op: &Op) -> Option<LOp> {
    let Op::CallPath {
        dst,
        path,
        base,
        argc,
    } = op
    else {
        return None;
    };
    let (segs, coerce) = &chunk.paths[*path as usize];
    if coerce.is_some() || *argc != 1 {
        return None;
    }
    let canon = vm.canonical(segs);
    let [ty, func] = canon.as_slice() else {
        return None;
    };
    if ty != "f64" || func != "from" {
        return None;
    }
    if vm.user_function("f64::from").is_some() || vm.user_method("f64", "from").is_some() {
        return None;
    }
    Some(LOp::F64From {
        dst: if *dst == u16::MAX {
            NO_SLOT
        } else {
            slot(regs, *dst)?
        },
        src: slot(regs, *base)?,
    })
}

/// The `Op::Method` arm of `translate`: a whitelisted numeric method whose
/// receiver and arguments read as slots.
fn translate_method(chunk: &Chunk, regs: &mut Vec<u16>, op: &Op) -> Option<LOp> {
    let Op::Method {
        dst,
        recv,
        name,
        base,
        argc,
    } = op
    else {
        return None;
    };
    let method = &chunk.names[*name as usize];
    let known = scalar_int_method(&method.text) || scalar_float_method(&method.text);
    if !known || method.scalar.is_some() || *argc > 2 {
        return None;
    }
    let mut args = [0u16; 2];
    for (arg, reg) in args.iter_mut().zip(*base..base.saturating_add(*argc)) {
        *arg = slot(regs, reg)?;
    }
    Some(LOp::NumMethod {
        dst: if *dst == u16::MAX {
            NO_SLOT
        } else {
            slot(regs, *dst)?
        },
        recv: slot(regs, *recv)?,
        args,
        argc: u8::try_from(*argc).ok()?,
        name: method.text.clone().into_boxed_str(),
    })
}

/// The vec-op and field-op arms of `translate`. They map only when the plan
/// carries a vec context, the while plan does, and the base register is one
/// of its vec bases, or, for a field op, one of its handle registers.
fn translate_vec(
    chunk: &Chunk,
    regs: &mut Vec<u16>,
    vecs: Option<&PlanVecs>,
    op: &Op,
) -> Option<LOp> {
    let ctx = vecs?;
    let vec_of = |r: u16| {
        ctx.bases
            .iter()
            .position(|&base| base == r)
            .and_then(|i| u16::try_from(i).ok())
    };
    let handle_of = |r: u16| {
        ctx.handles
            .iter()
            .position(|&h| h == r)
            .and_then(|i| u16::try_from(i).ok())
    };
    Some(match op {
        Op::Index { dst, base, key } | Op::UniqueIndex { dst, base, key } => {
            let unique = matches!(op, Op::UniqueIndex { .. });
            let vec = vec_of(*base)?;
            match handle_of(*dst) {
                Some(handle) => LOp::ElemRef {
                    handle,
                    vec,
                    idx: slot(regs, *key)?,
                    unique,
                },
                // The generic `UniqueIndex` also splits the element, which
                // is a no-op for the scalar elements `VecGet` can read, and
                // a non-scalar element fails the iteration over anyway.
                None => LOp::VecGet {
                    dst: slot(regs, *dst)?,
                    vec,
                    idx: slot(regs, *key)?,
                },
            }
        }
        Op::GetField { dst, base, member } | Op::UniqueField { dst, base, member } => {
            // The generic `UniqueField` also splits the field's storage,
            // which is a no-op for the scalar fields `FieldGet` can read.
            LOp::FieldGet {
                dst: slot(regs, *dst)?,
                handle: handle_of(*base)?,
                member: chunk.members[*member as usize].clone(),
            }
        }
        Op::SetField { base, member, val } => LOp::FieldSet {
            handle: handle_of(*base)?,
            member: chunk.members[*member as usize].clone(),
            val: slot(regs, *val)?,
        },
        Op::SetIndex { base, key, val } => {
            let vec = vec_of(*base)?;
            match handle_of(*val) {
                Some(handle) => LOp::ElemBack {
                    vec,
                    idx: slot(regs, *key)?,
                    handle,
                },
                None => LOp::VecSet {
                    vec,
                    idx: slot(regs, *key)?,
                    val: slot(regs, *val)?,
                },
            }
        }
        Op::UniqueReg { reg } => {
            vec_of(*reg)?;
            LOp::Nop
        }
        _ => return None,
    })
}

/// The one slot an op writes, for the move-folding pass. Jumps write none,
/// and neither does a method whose result the compiler discarded.
fn op_write(op: &LOp) -> Option<u16> {
    match op {
        LOp::LoadUnit { dst }
        | LOp::LoadInt { dst, .. }
        | LOp::LoadIntW { dst, .. }
        | LOp::LoadFloat { dst, .. }
        | LOp::LoadBool { dst, .. }
        | LOp::Move { dst, .. }
        | LOp::Bin { dst, .. }
        | LOp::BinImm { dst, .. }
        | LOp::Un { dst, .. }
        | LOp::Cast { dst, .. }
        | LOp::CastF64 { dst, .. }
        | LOp::VecGet { dst, .. }
        | LOp::FieldGet { dst, .. } => Some(*dst),
        LOp::NumMethod { dst, .. } | LOp::F64From { dst, .. } if *dst != NO_SLOT => Some(*dst),
        _ => None,
    }
}

/// Every slot an op reads, for the move-folding pass.
fn op_reads(op: &LOp, mut read: impl FnMut(u16)) {
    match op {
        LOp::Move { src, .. }
        | LOp::Un { a: src, .. }
        | LOp::Cast { src, .. }
        | LOp::CastF64 { src, .. }
        | LOp::F64From { src, .. } => read(*src),
        LOp::Bin { a, b, .. } | LOp::CmpJump { a, b, .. } => {
            read(*a);
            read(*b);
        }
        LOp::BinImm { a, .. } | LOp::CmpJumpImm { a, .. } => read(*a),
        LOp::JumpIfFalse { cond, .. } | LOp::JumpIfTrue { cond, .. } => read(*cond),
        LOp::NumMethod {
            recv, args, argc, ..
        } => {
            read(*recv);
            for arg in &args[..usize::from(*argc)] {
                read(*arg);
            }
        }
        LOp::VecGet { idx, .. } | LOp::ElemRef { idx, .. } | LOp::ElemBack { idx, .. } => {
            read(*idx);
        }
        LOp::VecSet { idx, val, .. } => {
            read(*idx);
            read(*val);
        }
        LOp::FieldSet { val, .. } => read(*val),
        LOp::LoadUnit { .. }
        | LOp::LoadInt { .. }
        | LOp::LoadIntW { .. }
        | LOp::LoadFloat { .. }
        | LOp::LoadBool { .. }
        | LOp::FieldGet { .. }
        | LOp::Jump { .. }
        | LOp::Nop => {}
    }
}

/// Retarget an op's write, for the move-folding pass.
fn set_write(op: &mut LOp, to: u16) {
    match op {
        LOp::LoadUnit { dst }
        | LOp::LoadInt { dst, .. }
        | LOp::LoadIntW { dst, .. }
        | LOp::LoadFloat { dst, .. }
        | LOp::LoadBool { dst, .. }
        | LOp::Move { dst, .. }
        | LOp::Bin { dst, .. }
        | LOp::BinImm { dst, .. }
        | LOp::Un { dst, .. }
        | LOp::Cast { dst, .. }
        | LOp::CastF64 { dst, .. }
        | LOp::F64From { dst, .. }
        | LOp::NumMethod { dst, .. }
        | LOp::VecGet { dst, .. }
        | LOp::FieldGet { dst, .. } => *dst = to,
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
pub(super) fn fold_moves(
    ops: &mut Vec<LOp>,
    val_slot: u16,
    frame_read: &[bool],
    slot_regs: &[u16],
) {
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
                    | LOp::LoadFloat { .. }
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
pub(super) fn build(vm: &Vm, chunk: &Chunk, head: usize) -> Option<LoopPlan> {
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
        .map(|op| {
            translate(
                vm,
                chunk,
                &Region {
                    head,
                    body: head + 1,
                    exit,
                },
                &mut regs,
                None,
                op,
            )
        })
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

/// What one op did: fall through, take a jump, or fail the iteration.
pub(super) enum OpOut {
    Fall,
    Jump(LTo),
    Fail,
}

/// The `NumMethod` arm of `eval_op`. Unused arg entries are slot zero,
/// which exists whenever a method op does, the receiver holds a slot
/// itself. The receiver picks the table, the same split the generic
/// dispatch makes between `int_methods` and `num_core`.
fn eval_num_method(
    regs: &[SVal],
    recv: u16,
    args: [u16; 2],
    count: u8,
    name: &str,
) -> Option<SVal> {
    let vals = [regs[usize::from(args[0])], regs[usize::from(args[1])]];
    let receiver = regs[usize::from(recv)];
    match receiver {
        SVal::Float(_) => s_float_method(name, receiver, &vals[..usize::from(count)]),
        _ => s_int_method(name, receiver, &vals[..usize::from(count)]),
    }
}

#[inline]
pub(super) fn eval_op(op: &LOp, regs: &mut [SVal]) -> OpOut {
    match op {
        LOp::LoadUnit { dst } => regs[usize::from(*dst)] = SVal::Unit,
        LOp::LoadInt { dst, v } => regs[usize::from(*dst)] = SVal::Int(*v),
        LOp::LoadIntW { dst, v, w } => regs[usize::from(*dst)] = SVal::IntW(*v, *w),
        LOp::LoadFloat { dst, v } => regs[usize::from(*dst)] = SVal::Float(*v),
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
        LOp::CastF64 { dst, src } => match s_cast_f64(regs[usize::from(*src)]) {
            Some(v) => regs[usize::from(*dst)] = v,
            None => return OpOut::Fail,
        },
        LOp::F64From { dst, src } => match s_f64_from(regs[usize::from(*src)]) {
            Some(v) => {
                if *dst != NO_SLOT {
                    regs[usize::from(*dst)] = v;
                }
            }
            None => return OpOut::Fail,
        },
        LOp::NumMethod {
            dst,
            recv,
            args,
            argc,
            name,
        } => match eval_num_method(regs, *recv, *args, *argc, name) {
            Some(v) => {
                if *dst != NO_SLOT {
                    regs[usize::from(*dst)] = v;
                }
            }
            None => return OpOut::Fail,
        },
        LOp::Nop => {}
        // Vec and field ops need the locked vec table and the handle table,
        // which only the journaled while runner holds, see
        // `scalar_while::run_vec_span`. They cannot appear in the plans the
        // other runners execute.
        LOp::VecGet { .. }
        | LOp::VecSet { .. }
        | LOp::ElemRef { .. }
        | LOp::FieldGet { .. }
        | LOp::FieldSet { .. }
        | LOp::ElemBack { .. } => return OpOut::Fail,
    }
    OpOut::Fall
}

/// Land the slots back in their frame registers. An `Opaque` slot was never
/// read or written, its register keeps the frame value it had.
pub(super) fn write_regs(ctx: &mut StepCtx, plan_regs: &[u16], regs: &[SVal]) {
    for (slot, &reg) in plan_regs.iter().enumerate() {
        if let Some(v) = s_value(regs[slot]) {
            ctx.put(reg, v);
        }
    }
}
