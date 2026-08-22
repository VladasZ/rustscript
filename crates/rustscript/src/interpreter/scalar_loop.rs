//! Scalar loop plans. A body that only moves plain scalars is translated once into a plan over
//! unboxed registers and runs inside 1 dispatch. A value the plan can't read is poison that aborts on
//! first read. Any failure rebuilds the registers to the start of the iteration and hands it to the
//! generic loop, so the panic lands on the exact op and line.
//!
//! This module has the plan IR, its translation and the op evaluator. The runners are `scalar_for`,
//! `scalar_while` and `scalar_fn`.

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use super::bytecode::{BinKind, BuiltinId, Member, PTag, UnKind};
use super::enum_def::EnumDef;
use super::numeric::IntWidth;
use super::scalar_val::TryFits;
use super::value::Value;

/// bounds the entry load and writeback cost
pub(super) const MAX_SLOTS: usize = 64;

pub(super) const MAX_CALL_ARGS: usize = 4;

pub(super) const MAX_ENUM_ARGS: usize = 4;

/// A discarded result, and the `val_slot` of a while plan. No real slot is ever this.
pub(super) const NO_SLOT: u16 = u16::MAX;

/// `Next` is the loop head, `Exit` the op after the loop.
#[derive(Clone, Copy)]
pub(super) enum LTo {
    Op(u32),
    Next,
    Exit,
}

/// Registers are dense plan slots, not frame registers.
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
    CastF64 {
        dst: u16,
        src: u16,
    },
    /// `f64::from(x)`, differs from the `as` cast, see `s_f64_from`
    F64From {
        dst: u16,
        src: u16,
    },
    /// `m.start()` or `m.end()` on a `Span` slot, any other receiver fails over
    MatchGet {
        dst: u16,
        recv: u16,
        end: bool,
    },
    /// `as_str`, `to_string` or `to_owned` on a span slot, see `s_as_str`. Any other receiver
    /// fails over.
    AsStr {
        dst: u16,
        src: u16,
    },
    /// integer `T::try_from(x)`, see `s_try_from`
    IntTryFrom {
        dst: u16,
        src: u16,
        fits: TryFits,
    },
    /// `.unwrap()` on an `OkInt` slot, any other receiver fails over
    UnwrapOk {
        dst: u16,
        src: u16,
    },
    /// A numeric method. The receiver picks `s_int_method` or `s_float_method` at run time. `dst`
    /// is `NO_SLOT` for a discarded result.
    NumMethod {
        dst: u16,
        recv: u16,
        args: [u16; 2],
        argc: u8,
        id: BuiltinId,
    },
    /// `dst = vec[idx]`. `vec` indexes the vec table, not a slot. A non scalar element or a bad
    /// index fails over.
    VecGet {
        dst: u16,
        vec: u16,
        idx: u16,
    },
    /// `vec[idx] = val`, journaled
    VecSet {
        vec: u16,
        idx: u16,
        val: u16,
    },
    /// The element `Arc` of `vec[idx]` into the handle table, split from sharing first for a
    /// `UniqueIndex`. A non struct element fails over.
    ElemRef {
        handle: u16,
        vec: u16,
        idx: u16,
        unique: bool,
    },
    /// `dst = handle.member`
    FieldGet {
        dst: u16,
        handle: u16,
        member: Member,
    },
    /// `handle.member = val`, journaled
    FieldSet {
        handle: u16,
        member: Member,
        val: u16,
    },
    /// the `SetIndex` writeback of a place chain, journaled
    ElemBack {
        vec: u16,
        idx: u16,
        handle: u16,
    },
    /// `vec.push(val)`, undo is a truncate to the entry length
    VecPush {
        vec: u16,
        val: u16,
    },
    /// `map.get(k).copied().unwrap_or(d)`, a non scalar hit fails over
    MapGetOr {
        dst: u16,
        map: u16,
        key: u16,
        default: u16,
    },
    /// `map.get(&k)` into a `SomeInt` or `NoneOpt` slot, a non int hit fails over
    MapGetOpt {
        dst: u16,
        map: u16,
        key: u16,
    },
    /// `map.contains_key(&k)`
    MapHas {
        dst: u16,
        map: u16,
        key: u16,
    },
    /// `map.insert(k, v)`, journaled. A kept old value that is not an int fails over.
    MapInsert {
        dst: u16,
        map: u16,
        key: u16,
        val: u16,
    },
    /// `Some(x)` against a `SomeInt` or `NoneOpt` slot. `bind` is untouched on a miss like the
    /// generic bind. Any other slot fails over.
    TestSome {
        dst: u16,
        val: u16,
        bind: u16,
    },
    /// a string literal into a `StrConst` slot, an `it["key"]` key
    LoadStr {
        dst: u16,
        id: u16,
    },
    /// `dst = item[key]` on an `Item` slot of the effects runner. A non map item, a missing key
    /// or a non scalar hit fails over.
    ItemIndex {
        dst: u16,
        item: u16,
        key: u16,
    },
    /// A `UniqueReg` on a vec base. The vec was split once at entry, so this only keeps its
    /// position for jump targets.
    Nop,
    /// The `::unreachable_match` call after a match. Fails over so the generic path reproduces
    /// the panic.
    FailOver,
    /// A user enum into the boxed table, built like `make_enum`. Function plans only, see `scalar_fn`.
    NewEnum {
        dst: u16,
        def: Arc<EnumDef>,
        variant: u16,
        args: [u16; MAX_ENUM_ARGS],
        argc: u8,
    },
    /// A unit variant into the boxed table, a clone of 1 prebuilt value. The shared empty payload
    /// splits on mutation anyway. Function plans only.
    UnitEnum {
        dst: u16,
        value: Value,
    },
    /// A unit or plain tuple variant pattern on a `Boxed` slot, mirrors the enum arms of
    /// `try_bind`. Any other slot fails over. Function plans only.
    TestVariant {
        dst: u16,
        val: u16,
        tag: PTag,
        binds: Box<[u16]>,
    },
    /// a recursive call into the same plan, see `scalar_fn`
    CallSelf {
        dst: u16,
        args: [u16; MAX_CALL_ARGS],
        argc: u8,
    },
    /// function plans only
    Ret {
        src: u16,
    },
}

pub struct LoopPlan {
    pub(super) ops: Vec<LOp>,
    pub(super) regs: Vec<u16>,
    /// The bases the body pushes into. Non empty plans run through the effects runner.
    pub(super) vecs: Vec<u16>,
    /// The maps the body probes, plus whether it inserts, which decides the entry split.
    pub(super) maps: Vec<u16>,
    pub(super) maps_written: Vec<bool>,
    pub(super) strs: Vec<Box<str>>,
    /// `ItemIndex` probes, only the effects runner can serve them
    pub(super) needs_items: bool,
    /// Runs that failed before 1 iteration. Past the budget the plan is dropped, so the loop
    /// stops paying the setup.
    pub(super) fails: AtomicU32,
    pub(super) val_slot: u16,
    /// 1 basic block, runs as a plain slice walk with no instruction pointer
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

/// The body of a `for` plan starts 1 past its `ForNext` head, a while plan at the head itself.
pub(super) struct Region {
    pub(super) head: usize,
    pub(super) body: usize,
    pub(super) exit: usize,
}

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

/// The vec bases and the handle registers of a while plan.
pub(super) struct PlanVecs<'a> {
    pub(super) bases: &'a [u16],
    pub(super) handles: &'a [u16],
}

mod eval;
mod translate;

pub(super) use eval::{OpOut, eval_op, write_regs};
pub(super) use translate::{MAX_PUSH_VECS, build, translate};
