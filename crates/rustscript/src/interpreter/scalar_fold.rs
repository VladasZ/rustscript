//! The move-folding cleanup pass shared by every scalar plan, see
//! `scalar_loop.rs` for the plan IR it rewrites.

use super::scalar_loop::{LOp, LTo, MAX_SLOTS, NO_SLOT};

/// The one slot an op writes, for the move-folding pass and the try-mask in
/// `translate`. Jumps write none, and neither does a method whose result
/// the compiler discarded.
pub(super) fn op_write(op: &LOp) -> Option<u16> {
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
        | LOp::CallSelf { dst, .. }
        | LOp::FieldGet { dst, .. } => Some(*dst),
        LOp::NumMethod { dst, .. }
        | LOp::F64From { dst, .. }
        | LOp::MatchGet { dst, .. }
        | LOp::IntTryFrom { dst, .. }
        | LOp::UnwrapOk { dst, .. }
            if *dst != NO_SLOT =>
        {
            Some(*dst)
        }
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
        | LOp::F64From { src, .. }
        | LOp::MatchGet { recv: src, .. }
        | LOp::IntTryFrom { src, .. }
        | LOp::UnwrapOk { src, .. }
        | LOp::Ret { src } => read(*src),
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
        LOp::CallSelf { args, argc, .. } => {
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
        | LOp::MatchGet { dst, .. }
        | LOp::IntTryFrom { dst, .. }
        | LOp::UnwrapOk { dst, .. }
        | LOp::VecGet { dst, .. }
        | LOp::CallSelf { dst, .. }
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
