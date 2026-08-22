//! Scalar loop plans. A body that only moves plain scalars is translated
//! once into a plan over unboxed registers and runs inside one dispatch. A
//! value the plan cannot read is poison that aborts on first read, and any
//! failure rebuilds the registers to the start of the iteration and hands it
//! to the generic loop, so the panic lands on the exact op and line.
//!
//! This module holds the plan IR, its translation and the op evaluator. The
//! runners are `scalar_for`, `scalar_while` and `scalar_fn`.

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

/// Bounds the entry load and writeback cost.
pub(super) const MAX_SLOTS: usize = 64;

/// See `scalar_fn`.
pub(super) const MAX_CALL_ARGS: usize = 4;

/// See `scalar_fn`.
pub(super) const MAX_ENUM_ARGS: usize = 4;

/// A discarded result, and the `val_slot` of a while plan. No real slot
/// reaches it.
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
    /// `f64::from(x)`, whose conversion differs from the `as` cast, see
    /// `s_f64_from`.
    F64From {
        dst: u16,
        src: u16,
    },
    /// `m.start()` or `m.end()` on a `Span` slot. Any other receiver fails
    /// over.
    MatchGet {
        dst: u16,
        recv: u16,
        end: bool,
    },
    /// `as_str`, `to_string` or `to_owned` on a span slot, see `s_as_str`.
    /// Any other receiver fails over.
    AsStr {
        dst: u16,
        src: u16,
    },
    /// Integer `T::try_from(x)`, see `s_try_from`.
    IntTryFrom {
        dst: u16,
        src: u16,
        fits: TryFits,
    },
    /// `.unwrap()` on an `OkInt` slot. Any other receiver fails over.
    UnwrapOk {
        dst: u16,
        src: u16,
    },
    /// A numeric method. The receiver picks `s_int_method` or
    /// `s_float_method` at run time. `dst` is `NO_SLOT` for a discarded
    /// result.
    NumMethod {
        dst: u16,
        recv: u16,
        args: [u16; 2],
        argc: u8,
        id: BuiltinId,
    },
    /// `dst = vec[idx]`. `vec` indexes the vec table, not a slot. A non
    /// scalar element or a bad index fails over.
    VecGet {
        dst: u16,
        vec: u16,
        idx: u16,
    },
    /// `vec[idx] = val`, journaled.
    VecSet {
        vec: u16,
        idx: u16,
        val: u16,
    },
    /// The element `Arc` of `vec[idx]` into the handle table, split from
    /// sharing first for a `UniqueIndex`. A non struct element fails over.
    ElemRef {
        handle: u16,
        vec: u16,
        idx: u16,
        unique: bool,
    },
    /// `dst = handle.member`.
    FieldGet {
        dst: u16,
        handle: u16,
        member: Member,
    },
    /// `handle.member = val`, journaled.
    FieldSet {
        handle: u16,
        member: Member,
        val: u16,
    },
    /// The `SetIndex` writeback of a place chain, journaled.
    ElemBack {
        vec: u16,
        idx: u16,
        handle: u16,
    },
    /// `vec.push(val)`. Undo is a truncate to the entry length.
    VecPush {
        vec: u16,
        val: u16,
    },
    /// `map.get(k).copied().unwrap_or(d)`. A non scalar hit fails over.
    MapGetOr {
        dst: u16,
        map: u16,
        key: u16,
        default: u16,
    },
    /// `map.get(&k)` into a `SomeInt` or `NoneOpt` slot. A non int hit fails
    /// over.
    MapGetOpt {
        dst: u16,
        map: u16,
        key: u16,
    },
    /// `map.contains_key(&k)`.
    MapHas {
        dst: u16,
        map: u16,
        key: u16,
    },
    /// `map.insert(k, v)`, journaled. A kept old value that is not an int
    /// fails over.
    MapInsert {
        dst: u16,
        map: u16,
        key: u16,
        val: u16,
    },
    /// `Some(x)` against a `SomeInt` or `NoneOpt` slot. `bind` is untouched
    /// on a miss like the generic bind. Any other slot fails over.
    TestSome {
        dst: u16,
        val: u16,
        bind: u16,
    },
    /// A string literal into a `StrConst` slot, an `it["key"]` key.
    LoadStr {
        dst: u16,
        id: u16,
    },
    /// `dst = item[key]` on an `Item` slot of the effects runner. A non map
    /// item, a missing key or a non scalar hit fails over.
    ItemIndex {
        dst: u16,
        item: u16,
        key: u16,
    },
    /// A `UniqueReg` on a vec base. The vec split once at entry, so this
    /// only keeps its position for jump targets.
    Nop,
    /// The `::unreachable_match` call after a match. Fails over so the
    /// generic path reproduces the panic.
    FailOver,
    /// A user enum into the boxed table, built like `make_enum`. Function
    /// plans only, see `scalar_fn`.
    NewEnum {
        dst: u16,
        def: Arc<EnumDef>,
        variant: u16,
        args: [u16; MAX_ENUM_ARGS],
        argc: u8,
    },
    /// A unit variant into the boxed table, a clone of one prebuilt value.
    /// The shared empty payload splits on mutation anyway. Function plans
    /// only.
    UnitEnum {
        dst: u16,
        value: Value,
    },
    /// A unit or plain tuple variant pattern on a `Boxed` slot, mirroring
    /// the enum arms of `try_bind`. Any other slot fails over. Function
    /// plans only.
    TestVariant {
        dst: u16,
        val: u16,
        tag: PTag,
        binds: Box<[u16]>,
    },
    /// A recursive call into the same plan, see `scalar_fn`.
    CallSelf {
        dst: u16,
        args: [u16; MAX_CALL_ARGS],
        argc: u8,
    },
    /// Function plans only.
    Ret {
        src: u16,
    },
}

pub struct LoopPlan {
    pub(super) ops: Vec<LOp>,
    pub(super) regs: Vec<u16>,
    /// The bases the body pushes into. Non empty plans run through the
    /// effects runner.
    pub(super) vecs: Vec<u16>,
    /// The maps the body probes, plus whether it inserts, which decides the
    /// entry split.
    pub(super) maps: Vec<u16>,
    pub(super) maps_written: Vec<bool>,
    pub(super) strs: Vec<Box<str>>,
    /// `ItemIndex` probes, which only the effects runner can serve.
    pub(super) needs_items: bool,
    /// Runs that failed before one iteration. Past the budget the plan is
    /// dropped, so the loop stops paying the setup.
    pub(super) fails: AtomicU32,
    pub(super) val_slot: u16,
    /// One basic block, which runs as a plain slice walk with no
    /// instruction pointer.
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

/// The `for` plan's body starts one past its `ForNext` head, the while
/// plan's at the head itself.
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

/// None rejects the whole loop. `vecs` is `None` for the `for` plan, which
/// rejects vec ops. `try_mask` has one bit per slot known to hold an
/// `IntTryFrom` result, the gate for `.unwrap()`.
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

/// Set by the conversion, carried by a move, cleared by any other write.
/// Only gates plan building, `UnwrapOk` checks the live slot anyway.
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
        // A deref of a plain value is a move. A real reference loads as
        // `Opaque` and moving one fails over.
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
        // A nested loop's entry hook only keeps its position.
        Op::LoopHead { .. } => LOp::Nop,
        _ => return None,
    })
}

/// Only `Some(x)` with a single plain binding maps, onto a `TestSome` that
/// mirrors `try_bind` on an Option.
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

/// Only `f64::from(x)` or an integer `T::try_from(x)` maps. A coercion on
/// the call site rejects the loop.
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

/// A numeric method, a match span accessor, or an `unwrap` of an
/// `IntTryFrom` result.
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
        match method.id {
            // Only a span receiver answers at run time, any other fails over.
            BuiltinId::AsStr | BuiltinId::ToString | BuiltinId::ToOwned => {
                let recv = slot(regs, *recv)?;
                return Some(LOp::AsStr {
                    dst: dst_slot(regs)?,
                    src: recv,
                });
            }
            BuiltinId::Start | BuiltinId::End => {
                let end = method.id == BuiltinId::End;
                let recv = slot(regs, *recv)?;
                return Some(LOp::MatchGet {
                    dst: dst_slot(regs)?,
                    recv,
                    end,
                });
            }
            // Only when the receiver statically holds an unwrappable plan
            // result and no user method shadows the builtin. The live slot
            // decides at run time.
            BuiltinId::Unwrap => {
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

/// Map only with a vec context and a base or handle register the plan
/// knows.
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
                // The element split of `UniqueIndex` is a no-op for scalars,
                // and a non scalar element fails over anyway.
                None => LOp::VecGet {
                    dst: slot(regs, *dst)?,
                    vec,
                    idx: slot(regs, *key)?,
                },
            }
        }
        Op::GetField { dst, base, member } | Op::UniqueField { dst, base, member } => {
            // The field split of `UniqueField` is a no-op for scalars.
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

/// Bounds the entry split and lock cost.
pub(super) const MAX_PUSH_VECS: usize = 4;

/// The push receivers in first appearance order. An extra argument, a kept
/// result or a user method shadowing the builtin rejects the loop.
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

/// Bounds the entry split and lock cost.
pub(super) const MAX_MAPS: usize = 4;

/// The map receivers in first appearance order, plus whether the body
/// inserts into each. Whether it really is a map only the runner's entry
/// check knows.
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
                // Any other method on a map base would fail every iteration,
                // so the loop stays generic.
                _ => None,
            }
        }
        _ => None,
    }
}

/// See `build`.
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
    /// The `UniqueReg` before a push or insert is the entry split the runner
    /// does once. Any other method on a base falls to `translate_op` and the
    /// role check in `build` rejects the plan.
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
            // Only the runner's probe ops read a `StrConst` slot.
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
            // `it["key"]`, the item slot holds the source position.
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

/// None when any op falls outside the subset.
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
    // A register cannot serve 2 tables at once, and a body that moves a
    // base around stays generic.
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

pub(super) enum OpOut {
    Fall,
    Jump(LTo),
    Fail,
}

/// Unused arg entries are slot zero, which always exists. The receiver
/// picks `int_methods` or `num_core`.
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

/// `None` fails over, a `NO_SLOT` dst discards the value.
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

/// Copying an `Opaque` would poison the destination and skip its
/// writeback, so it fails over like any other read of one.
#[inline]
fn eval_move(regs: &mut [SVal], dst: u16, src: u16) -> OpOut {
    let v = regs[usize::from(src)];
    if matches!(v, SVal::Opaque) {
        return OpOut::Fail;
    }
    regs[usize::from(dst)] = v;
    OpOut::Fall
}

/// Jump when the condition matches `want`, fail over on an `Opaque`.
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
// `scalar_fn::try_call`, which costs `binary_trees` about 7 percent and
// `collatz` about 12.
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
        // Vec and field ops belong to `scalar_while::run_vec_span`, map ops
        // to `scalar_for::run_effects`, enum and call ops to `scalar_fn`.
        // None can appear in the plans the other runners execute.
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

/// `Some(x)` against a map probe's answer, mirroring `try_bind`. Any other
/// value fails over.
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

/// An `Opaque` slot was never touched, its register keeps its value.
pub(super) fn write_regs(ctx: &mut StepCtx, plan_regs: &[u16], regs: &[SVal]) {
    for (slot, &reg) in plan_regs.iter().enumerate() {
        if let Some(v) = s_value(regs[slot]) {
            ctx.put(reg, v);
        }
    }
}
