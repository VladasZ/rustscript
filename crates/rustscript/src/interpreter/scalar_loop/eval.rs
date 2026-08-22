//! The plan op evaluator shared by every scalar runner.

use super::{LOp, LTo, NO_SLOT};
use crate::interpreter::bytecode::BuiltinId;
use crate::interpreter::scalar_val::{
    SVal, s_as_str, s_bin, s_cast, s_cast_f64, s_cmp, s_f64_from, s_float_method, s_int_method,
    s_match_get, s_try_from, s_un, s_unwrap_ok, s_value, truthy,
};
use crate::interpreter::vm_step::StepCtx;

pub(in crate::interpreter) enum OpOut {
    Fall,
    Jump(LTo),
    Fail,
}

/// Unused arg entries are slot zero, which always exists. The receiver picks `int_methods` or
/// `num_core`.
pub(super) fn eval_num_method(
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
pub(super) fn land(regs: &mut [SVal], dst: u16, v: Option<SVal>) -> OpOut {
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

/// Copying an `Opaque` would poison the destination and skip its writeback, so it fails over like
/// any other read of one.
#[inline]
pub(super) fn eval_move(regs: &mut [SVal], dst: u16, src: u16) -> OpOut {
    let v = regs[usize::from(src)];
    if matches!(v, SVal::Opaque) {
        return OpOut::Fail;
    }
    regs[usize::from(dst)] = v;
    OpOut::Fall
}

/// Jump when the condition matches `want`, fail over on an `Opaque`.
#[inline]
pub(super) fn eval_cond_jump(regs: &[SVal], cond: u16, to: LTo, want: bool) -> OpOut {
    if matches!(regs[usize::from(cond)], SVal::Opaque) {
        return OpOut::Fail;
    }
    if truthy(regs[usize::from(cond)]) == want {
        OpOut::Jump(to)
    } else {
        OpOut::Fall
    }
}

// Forced, not hinted. Left to the inliner it gets split out of `scalar_fn::try_call`, that costs
// `binary_trees` about 7 percent and `collatz` about 12.
#[inline(always)]
pub(in crate::interpreter) fn eval_op(op: &LOp, regs: &mut [SVal]) -> OpOut {
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
        // Vec and field ops belong to `scalar_while::run_vec_span`, map ops to
        // `scalar_for::run_effects`,
        // enum and call ops to `scalar_fn`. None of them can show up in the plans the other
        // runners execute.
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

/// `Some(x)` against a map probe result, mirrors `try_bind`. Any other value fails over.
#[inline]
pub(super) fn eval_test_some(regs: &mut [SVal], dst: u16, val: u16, bind: u16) -> OpOut {
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
pub(in crate::interpreter) fn write_regs(ctx: &mut StepCtx, plan_regs: &[u16], regs: &[SVal]) {
    for (slot, &reg) in plan_regs.iter().enumerate() {
        if let Some(v) = s_value(regs[slot]) {
            ctx.put(reg, v);
        }
    }
}
