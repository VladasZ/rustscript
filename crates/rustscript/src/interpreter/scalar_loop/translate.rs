//! Translation of a bytecode region into a scalar plan.

use std::sync::atomic::AtomicU32;

use super::{LOp, LTo, LoopPlan, NO_SLOT, PlanVecs, Region, slot, target};
use crate::interpreter::bytecode::{BuiltinId, Chunk, Const, Op, PPat, PathId};
use crate::interpreter::enum_def::{EnumKind, SOME};
use crate::interpreter::scalar_fold::{fold_moves, op_write};
use crate::interpreter::scalar_reads::chunk_reads;
use crate::interpreter::scalar_val::{scalar_float_method, scalar_int_method, try_fits_of};
use crate::interpreter::typeir::CastIr;
use crate::interpreter::vm::Vm;

/// None rejects the whole loop. `vecs` is `None` for the `for` plan, which rejects vec ops. `try_mask`
/// has 1 bit per slot known to hold an `IntTryFrom` result, the gate for `.unwrap()`.
pub(in crate::interpreter) fn translate(
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

/// Set by the conversion, carried by a move, cleared by any other write. Only gates plan
/// building, `UnwrapOk` checks the live slot anyway.
pub(super) fn update_try_mask(try_mask: &mut u64, lop: &LOp) {
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

pub(super) fn translate_op(
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
        // A deref of a plain value is a move. A real reference loads as `Opaque` and moving one
        // fails over.
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
        // the entry hook of a nested loop only keeps its position
        Op::LoopHead { .. } => LOp::Nop,
        _ => return None,
    })
}

/// Only `Some(x)` with a single plain binding maps, onto a `TestSome` that mirrors `try_bind` on
/// an Option.
pub(super) fn translate_test(
    chunk: &Chunk,
    regs: &mut Vec<u16>,
    val: u16,
    pat: u16,
    dst: u16,
) -> Option<LOp> {
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

/// Only `f64::from(x)` or an integer `T::try_from(x)` maps. A coercion on the call site rejects
/// the loop.
pub(super) fn translate_call(chunk: &Chunk, regs: &mut Vec<u16>, op: &Op) -> Option<LOp> {
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

/// A numeric method, a match span accessor, or an `unwrap` of an `IntTryFrom` result.
pub(super) fn translate_method(
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
            // only a span receiver works at run time, any other fails over
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
            // Only when the receiver statically holds an unwrappable plan result and no user method
            // shadows the builtin. The live slot decides at run time.
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

/// Maps only with a vec context and a base or handle register the plan knows.
pub(super) fn translate_vec(
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
                // the element split of `UniqueIndex` is a no-op for scalars, and a non scalar
                // element fails over anyway
                None => LOp::VecGet {
                    dst: slot(regs, *dst)?,
                    vec,
                    idx: slot(regs, *key)?,
                },
            }
        }
        Op::GetField { dst, base, member } | Op::UniqueField { dst, base, member } => {
            // the field split of `UniqueField` is a no-op for scalars
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

/// bounds the entry split and lock cost
pub(in crate::interpreter) const MAX_PUSH_VECS: usize = 4;

/// The push receivers in first appearance order. An extra argument, a kept result or a user
/// method shadowing the builtin rejects the loop.
pub(super) fn push_bases(vm: &Vm, chunk: &Chunk, body: usize, exit: usize) -> Option<Vec<u16>> {
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

/// bounds the entry split and lock cost
pub(in crate::interpreter) const MAX_MAPS: usize = 4;

/// The map receivers in first appearance order, plus whether the body inserts into each. Whether it
/// really is a map only the entry check of the runner knows.
pub(super) fn map_bases(chunk: &Chunk, body: usize, exit: usize) -> Option<(Vec<u16>, Vec<bool>)> {
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

pub(super) fn translate_map(
    chunk: &Chunk,
    regs: &mut Vec<u16>,
    maps: &[u16],
    op: &Op,
) -> Option<LOp> {
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
                // any other method on a map base would fail every iteration, so the loop stays generic
                _ => None,
            }
        }
        _ => None,
    }
}

/// See `build`.
pub(super) struct ForBuild<'a> {
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
    /// The `UniqueReg` before a push or insert is the entry split the runner does once. Any other
    /// method on a base falls to `translate_op` and the role check in `build` rejects the plan.
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
            // only the probe ops of the runner read a `StrConst` slot
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
            // `it["key"]`, the item slot holds the source position
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
pub(in crate::interpreter) fn build(vm: &Vm, chunk: &Chunk, head: usize) -> Option<LoopPlan> {
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
    // A register can't serve 2 tables at once, and a body that moves a base around stays generic.
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
