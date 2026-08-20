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
//! from bytecode, and the op evaluator. The move-folding cleanup lives in
//! `scalar_fold`, and the runners in `scalar_for`, `scalar_while`, and
//! `scalar_fn`.

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use super::bytecode::{BinKind, BuiltinId, Chunk, Const, Member, Op, PPat, PTag, PathId, UnKind};
use super::enum_def::{EnumDef, EnumKind, SOME};
use super::numeric::IntWidth;
use super::scalar_fold::{fold_moves, op_write};
use super::scalar_reads::chunk_reads;
use super::scalar_val::{
    SVal, TryFits, s_as_str, s_bin, s_cast, s_cast_f64, s_cmp, s_f64_from, s_float_method,
    s_int_method, s_match_get, s_try_from, s_un, s_unwrap_ok, s_value, scalar_float_method,
    scalar_int_method, truthy, try_fits_of,
};
use super::typeir::CastIr;
use super::value::Value;
use super::vm::Vm;
use super::vm_step::StepCtx;

/// Register cap per plan, bounding the entry load and writeback cost.
pub(super) const MAX_SLOTS: usize = 64;

/// Argument slots a plan's self call carries, see `scalar_fn`.
pub(super) const MAX_CALL_ARGS: usize = 4;

/// Payload slots a plan's `NewEnum` carries, see `scalar_fn`.
pub(super) const MAX_ENUM_ARGS: usize = 4;

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
    /// `m.start()` or `m.end()` on a `Span` slot, a regex match item of a
    /// `find_iter` chunk. Any other receiver fails the iteration over to
    /// the generic path, whose own dispatch answers it.
    MatchGet {
        dst: u16,
        recv: u16,
        end: bool,
    },
    /// `s.as_str()`, `s.to_string()`, or `s.to_owned()` on a span slot,
    /// answering the same slice as a `StrSpan`, see `s_as_str`. Any other
    /// receiver fails the iteration over to the generic path.
    AsStr {
        dst: u16,
        src: u16,
    },
    /// An unshadowed integer `T::try_from(x)` call whose fitting value
    /// answers an `OkInt` slot, see `s_try_from`.
    IntTryFrom {
        dst: u16,
        src: u16,
        fits: TryFits,
    },
    /// `.unwrap()` on an `OkInt` slot, the dst of an earlier `IntTryFrom`.
    /// Any other receiver fails the iteration over to the generic path.
    UnwrapOk {
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
        id: BuiltinId,
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
    /// `vec.push(val)` on one of a for plan's locked vecs. `vec` indexes the
    /// plan's vec table. Undo for a failing iteration is a truncate back to
    /// the iteration's entry length, pushes only append.
    VecPush {
        vec: u16,
        val: u16,
    },
    /// The fused `map.get(k).copied().unwrap_or(d)` on one of a for plan's
    /// locked maps. `map` indexes the plan's map table. A non-scalar hit
    /// fails the iteration over to the generic path.
    MapGetOr {
        dst: u16,
        map: u16,
        key: u16,
        default: u16,
    },
    /// `map.get(&k)` on a locked map, answering a `SomeInt` or `NoneOpt`
    /// slot. A hit whose value is not a plain int fails the iteration over.
    MapGetOpt {
        dst: u16,
        map: u16,
        key: u16,
    },
    /// `map.contains_key(&k)` on a locked map.
    MapHas {
        dst: u16,
        map: u16,
        key: u16,
    },
    /// `map.insert(k, v)` on a locked map, journaled so a failing iteration
    /// can undo it. `dst` is `NO_SLOT` when the compiler discarded the old
    /// value, and a kept old value that is not a plain int fails over.
    MapInsert {
        dst: u16,
        map: u16,
        key: u16,
        val: u16,
    },
    /// A `TestBind` against the pattern `Some(x)` on a `SomeInt` or
    /// `NoneOpt` slot: `dst` gets the match flag, and `bind` gets the
    /// payload on a match, untouched otherwise like the generic bind. Any
    /// other tested slot fails the iteration over to the generic path.
    TestSome {
        dst: u16,
        val: u16,
        bind: u16,
    },
    /// A string literal into a `StrConst` slot naming the plan's string
    /// table entry, an `it["key"]` key for one.
    LoadStr {
        dst: u16,
        id: u16,
    },
    /// `dst = item[key]` where `item` is an `Item` slot of the effects
    /// runner's source walk, a parsed json object for one. The runner
    /// probes the boxed map item; a non-map item, a missing key, or a
    /// non-scalar hit fails the iteration over to the generic path.
    ItemIndex {
        dst: u16,
        item: u16,
        key: u16,
    },
    /// A `UniqueReg` on a vec base. The plan split the vec from sharing
    /// once at entry, so per-write splits inside the loop have nothing to
    /// do, but the op keeps its position for jump targets.
    Nop,
    /// The compiler-internal `::unreachable_match` call after a match's
    /// arms. Reached only when no arm matched, and fails the run over so
    /// the generic path reproduces its exact panic.
    FailOver,
    /// A user enum value into the function runner's boxed table, built the
    /// way the generic `make_enum` builds one: fresh list storage holding
    /// the payload slots' values. Only in function plans, whose runner
    /// holds the table, see `scalar_fn`.
    NewEnum {
        dst: u16,
        def: Arc<EnumDef>,
        variant: u16,
        args: [u16; MAX_ENUM_ARGS],
        argc: u8,
    },
    /// A unit variant value into the boxed table, a clone of one prebuilt
    /// value per op. The clone shares the empty payload storage, which
    /// mutation would split from anyway, so it is indistinguishable from
    /// the fresh value `load_enum` builds. Only in function plans.
    UnitEnum {
        dst: u16,
        value: Value,
    },
    /// A `TestBind` against a unit variant pattern or a tuple variant whose
    /// elements are all plain bindings, on a `Boxed` enum slot: `dst` gets
    /// the match flag, and the payload lands in `binds` in order on a
    /// match, untouched otherwise, mirroring the enum arms of the generic
    /// `try_bind`. Any other tested slot fails the run over to the generic
    /// path. Only in function plans, see `scalar_fn`.
    TestVariant {
        dst: u16,
        val: u16,
        tag: PTag,
        binds: Box<[u16]>,
    },
    /// A recursive call back into the same function plan, run by the
    /// `scalar_fn` runner on its own frame stack.
    CallSelf {
        dst: u16,
        args: [u16; MAX_CALL_ARGS],
        argc: u8,
    },
    /// A function body's `Ret`, only in function plans, see `scalar_fn`.
    Ret {
        src: u16,
    },
}

pub struct LoopPlan {
    pub(super) ops: Vec<LOp>,
    /// The frame register behind each plan slot.
    pub(super) regs: Vec<u16>,
    /// The frame register behind each vec table entry, the bases the body
    /// pushes into. Non-empty plans run through the effects runner, which
    /// locks each base's storage for the chunk.
    pub(super) vecs: Vec<u16>,
    /// The frame register behind each map table entry, the maps the body
    /// probes or inserts into, plus whether the body inserts, which decides
    /// the entry split. Non-empty plans run through the effects runner.
    pub(super) maps: Vec<u16>,
    pub(super) maps_written: Vec<bool>,
    /// The plan's string constants, the table `StrConst` slots index.
    pub(super) strs: Vec<Box<str>>,
    /// Whether the body probes loop items in place through `ItemIndex`,
    /// which only the effects runner can serve.
    pub(super) needs_items: bool,
    /// Effects-runner runs that failed before finishing one iteration. Past
    /// the budget the plan is dropped from the cache, so a loop whose entry
    /// state never runs as an effects plan stops paying the setup per
    /// iteration.
    pub(super) fails: AtomicU32,
    /// The slot of the `ForNext` value register, written per item.
    pub(super) val_slot: u16,
    /// True when the body is one basic block: no jump ops except the final
    /// `Jump` back to the head. Such a body runs as a plain slice walk with
    /// no instruction pointer.
    pub(super) straight: bool,
}

pub(super) fn slot(regs: &mut Vec<u16>, r: u16) -> Option<u16> {
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
/// the `for` plan, which rejects vec ops. `try_mask` carries one bit per
/// slot statically known to hold an `IntTryFrom` result at this point of
/// the linear op walk, the gate for translating `.unwrap()`.
pub(super) fn translate(
    vm: &Vm,
    chunk: &Chunk,
    region: &Region,
    regs: &mut Vec<u16>,
    vecs: Option<&PlanVecs>,
    try_mask: &mut u64,
    op: &Op,
) -> Option<LOp> {
    let lop = translate_op(vm, chunk, region, regs, vecs, *try_mask, op)?;
    update_try_mask(try_mask, &lop);
    Some(lop)
}

/// Track which slots hold an `IntTryFrom` result through the linear op
/// walk: set by the conversion, carried by a move, cleared by any other
/// write. The mask only gates plan building, the `UnwrapOk` op checks the
/// live slot at run time either way.
fn update_try_mask(try_mask: &mut u64, lop: &LOp) {
    let bit = |slot: u16| 1u64.checked_shl(u32::from(slot)).unwrap_or(0);
    match lop {
        LOp::IntTryFrom { dst, .. } | LOp::NumMethod { dst, .. } | LOp::MapGetOpt { dst, .. }
            if *dst != NO_SLOT =>
        {
            *try_mask |= bit(*dst);
        }
        LOp::Move { dst, src } => {
            if *try_mask & bit(*src) != 0 {
                *try_mask |= bit(*dst);
            } else {
                *try_mask &= !bit(*dst);
            }
        }
        _ => {
            if let Some(dst) = op_write(lop) {
                *try_mask &= !bit(dst);
            }
        }
    }
}

fn translate_op(
    vm: &Vm,
    chunk: &Chunk,
    region: &Region,
    regs: &mut Vec<u16>,
    vecs: Option<&PlanVecs>,
    try_mask: u64,
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
        // or cell loads as `Opaque`, and moving one fails the iteration
        // over to the generic path, which derefs for real.
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
        Op::Method { .. } => return translate_method(vm, chunk, regs, try_mask, op),
        Op::CallPath { .. } => return translate_call(chunk, regs, op),
        Op::TestBind { val, pat, dst } => return translate_test(chunk, regs, *val, *pat, *dst),
        // A nested loop's entry hook has nothing to do inside a plan, which
        // already runs the nested loop unboxed, but keeps its position.
        Op::LoopHead { .. } => LOp::Nop,
        _ => return None,
    })
}

/// The `Op::TestBind` arm of `translate`: only the pattern `Some(x)` with a
/// single plain binding maps, onto a `TestSome` whose run-time check keeps
/// any other tested value on the generic path. The plan op mirrors the
/// generic `try_bind` on an Option exactly: flag plus payload on a match,
/// flag alone otherwise.
fn translate_test(chunk: &Chunk, regs: &mut Vec<u16>, val: u16, pat: u16, dst: u16) -> Option<LOp> {
    let info = &chunk.pats[pat as usize];
    let PPat::TupleStruct { tag, elems } = &info.pat else {
        return None;
    };
    let [
        PPat::Ident {
            name: elem,
            sub: None,
        },
    ] = elems.as_slice()
    else {
        return None;
    };
    let [(bind, reg)] = info.binds.as_slice() else {
        return None;
    };
    let is_some = matches!(&tag.variant, Some((def, SOME)) if def.kind == EnumKind::Option);
    if !is_some || bind != elem {
        return None;
    }
    Some(LOp::TestSome {
        dst: slot(regs, dst)?,
        val: slot(regs, val)?,
        bind: slot(regs, *reg)?,
    })
}

/// The `Op::CallPath` arm of `translate`: only a plain `f64::from(x)` or an
/// integer `T::try_from(x)` call maps. A coercion on the call site has no
/// plan equivalent, so it rejects the loop.
fn translate_call(chunk: &Chunk, regs: &mut Vec<u16>, op: &Op) -> Option<LOp> {
    let Op::CallPath {
        dst,
        path,
        base,
        argc,
    } = op
    else {
        return None;
    };
    let path = &chunk.paths[*path as usize];
    if path.id == PathId::UnreachableMatch {
        return Some(LOp::FailOver);
    }
    if path.coerce.is_some() || *argc != 1 {
        return None;
    }
    let dst = if *dst == u16::MAX {
        NO_SLOT
    } else {
        slot(regs, *dst)?
    };
    let src = slot(regs, *base)?;
    if path.id == PathId::F64From {
        return Some(LOp::F64From { dst, src });
    }
    if path.id.name() == "try_from" {
        let fits = try_fits_of(path.id.namespace())?;
        return Some(LOp::IntTryFrom { dst, src, fits });
    }
    None
}

/// The `Op::Method` arm of `translate`: a whitelisted numeric method whose
/// receiver and arguments read as slots, a match span accessor, or an
/// `unwrap` of an `IntTryFrom` result.
fn translate_method(
    vm: &Vm,
    chunk: &Chunk,
    regs: &mut Vec<u16>,
    try_mask: u64,
    op: &Op,
) -> Option<LOp> {
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
    if method.scalar.is_none() && *argc == 0 {
        let dst_slot = |regs: &mut Vec<u16>| {
            if *dst == u16::MAX {
                Some(NO_SLOT)
            } else {
                slot(regs, *dst)
            }
        };
        match method.text.as_str() {
            // Only a span receiver answers at run time, so a same-named
            // method on any other receiver fails its iteration over to the
            // generic path, whose own dispatch resolves it.
            "as_str" | "to_string" | "to_owned" => {
                let recv = slot(regs, *recv)?;
                return Some(LOp::AsStr {
                    dst: dst_slot(regs)?,
                    src: recv,
                });
            }
            "start" | "end" => {
                let end = method.text.as_str() == "end";
                let recv = slot(regs, *recv)?;
                return Some(LOp::MatchGet {
                    dst: dst_slot(regs)?,
                    recv,
                    end,
                });
            }
            // Only when the receiver statically holds an unwrappable plan
            // result, an `IntTryFrom`, a checked numeric method, or a map
            // probe, so an `unwrap` on anything else keeps rejecting the
            // loop, and only while no user method on `Result` or `Option`
            // shadows the builtin. The live slot decides at run time.
            "unwrap" => {
                let recv = slot(regs, *recv)?;
                if try_mask & 1u64.checked_shl(u32::from(recv)).unwrap_or(0) == 0
                    || vm.user_method("Result", "unwrap").is_some()
                    || vm.user_method("Option", "unwrap").is_some()
                {
                    return None;
                }
                return Some(LOp::UnwrapOk {
                    dst: dst_slot(regs)?,
                    src: recv,
                });
            }
            _ => {}
        }
    }
    let known = scalar_int_method(method.id) || scalar_float_method(method.id);
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
        id: method.id,
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

/// Vec table cap for a for plan's push bases, bounding the entry split and
/// lock cost.
pub(super) const MAX_PUSH_VECS: usize = 4;

/// The body's push receivers in first-appearance order, the vec table the
/// plan's `VecPush` ops index. A push shape the plan cannot run, an extra
/// argument, a kept result, or a user method shadowing the builtin, rejects
/// the whole loop.
fn push_bases(vm: &Vm, chunk: &Chunk, body: usize, exit: usize) -> Option<Vec<u16>> {
    let mut bases: Vec<u16> = Vec::new();
    for op in &chunk.code[body..exit] {
        let Op::Method {
            dst,
            recv,
            name,
            argc,
            ..
        } = op
        else {
            continue;
        };
        let name = &chunk.names[*name as usize];
        if name.id != BuiltinId::Push {
            continue;
        }
        if *argc != 1 || *dst != u16::MAX || name.scalar.is_some() {
            return None;
        }
        if !bases.contains(recv) {
            if bases.len() >= MAX_PUSH_VECS {
                return None;
            }
            bases.push(*recv);
        }
    }
    if !bases.is_empty() && vm.user_method("Vec", "push").is_some() {
        return None;
    }
    Some(bases)
}

/// Map table cap for a for plan's probed maps, bounding the entry split and
/// lock cost.
pub(super) const MAX_MAPS: usize = 4;

/// The body's map receivers in first-appearance order, the map table the
/// plan's map ops index, plus whether the body inserts into each. Every
/// `GetOrDefault`, `get`, `contains_key`, and two-argument `insert` receiver
/// counts: whether it really is a map only the runner's entry check knows,
/// and a base that is not one fails the run over before any iteration.
fn map_bases(chunk: &Chunk, body: usize, exit: usize) -> Option<(Vec<u16>, Vec<bool>)> {
    let mut bases: Vec<u16> = Vec::new();
    let mut written: Vec<bool> = Vec::new();
    for op in &chunk.code[body..exit] {
        let (base, writes) = match op {
            Op::GetOrDefault { recv, .. } => (*recv, false),
            Op::Method {
                dst,
                recv,
                name,
                argc,
                ..
            } => {
                let name = &chunk.names[*name as usize];
                if name.scalar.is_some() {
                    continue;
                }
                match name.id {
                    BuiltinId::Insert if *argc == 2 => (*recv, true),
                    BuiltinId::Get | BuiltinId::ContainsKey if *argc == 1 && *dst != u16::MAX => {
                        (*recv, false)
                    }
                    _ => continue,
                }
            }
            _ => continue,
        };
        if let Some(i) = bases.iter().position(|&r| r == base) {
            written[i] = written[i] || writes;
            continue;
        }
        if bases.len() >= MAX_MAPS {
            return None;
        }
        bases.push(base);
        written.push(writes);
    }
    Some((bases, written))
}

/// Translate one map op of the `build` closure below, whose receiver the
/// base scan already put in the map table.
fn translate_map(chunk: &Chunk, regs: &mut Vec<u16>, maps: &[u16], op: &Op) -> Option<LOp> {
    let map_of = |r: u16| {
        maps.iter()
            .position(|&base| base == r)
            .and_then(|i| u16::try_from(i).ok())
    };
    match op {
        Op::GetOrDefault {
            dst,
            recv,
            key,
            default,
        } => Some(LOp::MapGetOr {
            dst: slot(regs, *dst)?,
            map: map_of(*recv)?,
            key: slot(regs, *key)?,
            default: slot(regs, *default)?,
        }),
        Op::Method {
            dst,
            recv,
            name,
            base,
            argc,
        } => {
            let map = map_of(*recv)?;
            let name = &chunk.names[*name as usize];
            let dst_slot = |regs: &mut Vec<u16>| {
                if *dst == u16::MAX {
                    Some(NO_SLOT)
                } else {
                    slot(regs, *dst)
                }
            };
            match name.id {
                BuiltinId::Insert if *argc == 2 => Some(LOp::MapInsert {
                    dst: dst_slot(regs)?,
                    map,
                    key: slot(regs, *base)?,
                    val: slot(regs, base.checked_add(1)?)?,
                }),
                BuiltinId::Get if *argc == 1 && *dst != u16::MAX => Some(LOp::MapGetOpt {
                    dst: slot(regs, *dst)?,
                    map,
                    key: slot(regs, *base)?,
                }),
                BuiltinId::ContainsKey if *argc == 1 && *dst != u16::MAX => Some(LOp::MapHas {
                    dst: slot(regs, *dst)?,
                    map,
                    key: slot(regs, *base)?,
                }),
                // Any other method on a map base, a `len` or an `iter`, has
                // no plan op, and slotting the base as a scalar would only
                // fail every iteration, so the loop stays generic.
                _ => None,
            }
        }
        _ => None,
    }
}

/// The mutable state the for-plan translation threads through its linear
/// op walk, see `build`.
struct ForBuild<'a> {
    vm: &'a Vm,
    chunk: &'a Chunk,
    region: Region,
    bases: &'a [u16],
    maps: &'a [u16],
    val: u16,
    regs: Vec<u16>,
    strs: Vec<Box<str>>,
    try_mask: u64,
}

impl ForBuild<'_> {
    /// Translate one body op. The push shape was checked by the base scan,
    /// and the generic `UniqueReg` before a push or an insert is the entry
    /// split the effects runner performs once. Any other method on a base
    /// falls to `translate_op`, which slots the receiver, and the role
    /// check in `build` rejects the plan.
    fn translate(&mut self, op: &Op) -> Option<LOp> {
        let lop = match op {
            Op::Method {
                recv, base, name, ..
            } if self.bases.contains(recv)
                && self.chunk.names[*name as usize].id == BuiltinId::Push =>
            {
                Some(LOp::VecPush {
                    vec: u16::try_from(self.bases.iter().position(|r| r == recv)?).ok()?,
                    val: slot(&mut self.regs, *base)?,
                })
            }
            Op::UniqueReg { reg } if self.bases.contains(reg) || self.maps.contains(reg) => {
                Some(LOp::Nop)
            }
            Op::GetOrDefault { recv, .. } | Op::Method { recv, .. } if self.maps.contains(recv) => {
                translate_map(self.chunk, &mut self.regs, self.maps, op)
            }
            // A string literal, an `it["key"]` key for one, into the plan's
            // string table. Only the runner's probe ops read the slot,
            // anything else fails its iteration over.
            Op::LoadConst { dst, k }
                if matches!(&self.chunk.consts[*k as usize], Const::Str(_)) =>
            {
                let Const::Str(text) = &self.chunk.consts[*k as usize] else {
                    return None;
                };
                let id = u16::try_from(self.strs.len()).ok()?;
                self.strs.push(Box::from(&**text));
                Some(LOp::LoadStr {
                    dst: slot(&mut self.regs, *dst)?,
                    id,
                })
            }
            // Indexing the loop item itself, the json shape of `it["key"]`.
            // The item slot holds the source position the effects runner
            // probes the boxed item through.
            Op::Index { dst, base, key } if *base == self.val => Some(LOp::ItemIndex {
                dst: slot(&mut self.regs, *dst)?,
                item: slot(&mut self.regs, *base)?,
                key: slot(&mut self.regs, *key)?,
            }),
            other => translate_op(
                self.vm,
                self.chunk,
                &self.region,
                &mut self.regs,
                None,
                self.try_mask,
                other,
            ),
        }?;
        update_try_mask(&mut self.try_mask, &lop);
        Some(lop)
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
    let bases = push_bases(vm, chunk, head + 1, exit)?;
    let (maps, maps_written) = map_bases(chunk, head + 1, exit)?;
    if maps.iter().any(|m| bases.contains(m)) {
        return None;
    }
    let mut build = ForBuild {
        vm,
        chunk,
        region: Region {
            head,
            body: head + 1,
            exit,
        },
        bases: &bases,
        maps: &maps,
        val: *val,
        regs: Vec::new(),
        strs: Vec::new(),
        try_mask: 0,
    };
    let val_slot = slot(&mut build.regs, *val)?;
    let mut ops = chunk.code[head + 1..exit]
        .iter()
        .map(|op| build.translate(op))
        .collect::<Option<Vec<_>>>()?;
    let (regs, strs) = (build.regs, build.strs);
    // A register cannot serve two tables at once: a locked pushed vec, a
    // locked map, and a scalar slot are disjoint roles, and a body that
    // also moves a base around stays generic.
    if regs
        .iter()
        .any(|reg| bases.contains(reg) || maps.contains(reg))
    {
        return None;
    }
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
    let needs_items = ops.iter().any(|op| matches!(op, LOp::ItemIndex { .. }));
    Some(LoopPlan {
        ops,
        regs,
        vecs: bases,
        maps,
        maps_written,
        strs,
        needs_items,
        fails: AtomicU32::new(0),
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
    id: BuiltinId,
) -> Option<SVal> {
    let vals = [regs[usize::from(args[0])], regs[usize::from(args[1])]];
    let receiver = regs[usize::from(recv)];
    match receiver {
        SVal::Float(_) => s_float_method(id, receiver, &vals[..usize::from(count)]),
        _ => s_int_method(id, receiver, &vals[..usize::from(count)]),
    }
}

/// Land a method or conversion result: `None` fails the iteration over to
/// the generic path, and a `NO_SLOT` dst discards the value the way the
/// compiler discarded the generic result.
#[inline]
fn land(regs: &mut [SVal], dst: u16, v: Option<SVal>) -> OpOut {
    match v {
        Some(v) => {
            if dst != NO_SLOT {
                regs[usize::from(dst)] = v;
            }
            OpOut::Fall
        }
        None => OpOut::Fail,
    }
}

/// The `Move` arm of `eval_op`. Copying an `Opaque` would poison the
/// destination, and a poisoned slot skips writeback, so its frame register
/// would keep the value it held before the loop instead of the copy. Fail
/// over the way any other read of an `Opaque` does.
#[inline]
fn eval_move(regs: &mut [SVal], dst: u16, src: u16) -> OpOut {
    let v = regs[usize::from(src)];
    if matches!(v, SVal::Opaque) {
        return OpOut::Fail;
    }
    regs[usize::from(dst)] = v;
    OpOut::Fall
}

/// The conditional-jump arms of `eval_op`: jump when the condition's truth
/// matches `want`, and fail over on an `Opaque` condition the way any other
/// read of one does.
#[inline]
fn eval_cond_jump(regs: &[SVal], cond: u16, to: LTo, want: bool) -> OpOut {
    if matches!(regs[usize::from(cond)], SVal::Opaque) {
        return OpOut::Fail;
    }
    if truthy(regs[usize::from(cond)]) == want {
        OpOut::Jump(to)
    } else {
        OpOut::Fall
    }
}

// Forced, not hinted. Left to the inliner it gets split out of
// `scalar_fn::try_call` whenever the surrounding code changes size, which
// costs `binary_trees` about 7 percent and `collatz` about 12.
#[inline(always)]
pub(super) fn eval_op(op: &LOp, regs: &mut [SVal]) -> OpOut {
    match op {
        LOp::LoadUnit { dst } => regs[usize::from(*dst)] = SVal::Unit,
        LOp::LoadInt { dst, v } => regs[usize::from(*dst)] = SVal::Int(*v),
        LOp::LoadIntW { dst, v, w } => regs[usize::from(*dst)] = SVal::IntW(*v, *w),
        LOp::LoadFloat { dst, v } => regs[usize::from(*dst)] = SVal::Float(*v),
        LOp::LoadBool { dst, v } => regs[usize::from(*dst)] = SVal::Bool(*v),
        LOp::Move { dst, src } => return eval_move(regs, *dst, *src),
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
        LOp::Un { dst, a, op } => return land(regs, *dst, s_un(*op, regs[usize::from(*a)])),
        LOp::Jump { to } => return OpOut::Jump(*to),
        LOp::JumpIfFalse { cond, to } => return eval_cond_jump(regs, *cond, *to, false),
        LOp::JumpIfTrue { cond, to } => return eval_cond_jump(regs, *cond, *to, true),
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
        LOp::Cast { dst, src, w } => {
            return land(regs, *dst, s_cast(regs[usize::from(*src)], *w));
        }
        LOp::CastF64 { dst, src } => {
            return land(regs, *dst, s_cast_f64(regs[usize::from(*src)]));
        }
        LOp::F64From { dst, src } => {
            let v = s_f64_from(regs[usize::from(*src)]);
            return land(regs, *dst, v);
        }
        LOp::MatchGet { dst, recv, end } => {
            return land(regs, *dst, s_match_get(regs[usize::from(*recv)], *end));
        }
        LOp::AsStr { dst, src } => {
            return land(regs, *dst, s_as_str(regs[usize::from(*src)]));
        }
        LOp::IntTryFrom { dst, src, fits } => {
            return land(regs, *dst, s_try_from(*fits, regs[usize::from(*src)]));
        }
        LOp::UnwrapOk { dst, src } => {
            return land(regs, *dst, s_unwrap_ok(regs[usize::from(*src)]));
        }
        LOp::NumMethod {
            dst,
            recv,
            args,
            argc,
            id,
        } => {
            let v = eval_num_method(regs, *recv, *args, *argc, *id);
            return land(regs, *dst, v);
        }
        LOp::Nop => {}
        LOp::TestSome { dst, val, bind } => return eval_test_some(regs, *dst, *val, *bind),
        LOp::LoadStr { dst, id } => regs[usize::from(*dst)] = SVal::StrConst(*id),
        // Vec and field ops need the locked vec table and the handle table,
        // which only the journaled while runner holds, see
        // `scalar_while::run_vec_span`. Map ops need the locked map table
        // only the effects runner holds, see `scalar_for::run_effects`.
        // Enum, call, and return ops need the boxed table and the frame
        // stack only the `scalar_fn` runner holds. None of them can appear
        // in the plans the other runners execute.
        LOp::VecGet { .. }
        | LOp::VecSet { .. }
        | LOp::VecPush { .. }
        | LOp::MapGetOr { .. }
        | LOp::MapGetOpt { .. }
        | LOp::MapHas { .. }
        | LOp::MapInsert { .. }
        | LOp::ItemIndex { .. }
        | LOp::ElemRef { .. }
        | LOp::FieldGet { .. }
        | LOp::FieldSet { .. }
        | LOp::ElemBack { .. }
        | LOp::NewEnum { .. }
        | LOp::UnitEnum { .. }
        | LOp::TestVariant { .. }
        | LOp::CallSelf { .. }
        | LOp::Ret { .. }
        | LOp::FailOver => return OpOut::Fail,
    }
    OpOut::Fall
}

/// The `TestSome` arm of `eval_op`: the pattern `Some(x)` against a scalar
/// map probe's answer, mirroring the generic `try_bind` on an Option
/// exactly. The binding lands only on a match, and any other tested value
/// fails the iteration over to the generic path.
#[inline]
fn eval_test_some(regs: &mut [SVal], dst: u16, val: u16, bind: u16) -> OpOut {
    match regs[usize::from(val)] {
        SVal::SomeInt(n) => {
            regs[usize::from(bind)] = SVal::Int(n);
            regs[usize::from(dst)] = SVal::Bool(true);
        }
        SVal::NoneOpt => regs[usize::from(dst)] = SVal::Bool(false),
        _ => return OpOut::Fail,
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
