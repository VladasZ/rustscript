//! Which frame registers a chunk's ops read, for the dead-store removal in
//! `scalar_loop.rs`. A register no op anywhere in the chunk reads can absorb
//! a dead store: skipping the write is unobservable.

use super::bytecode::{CapSource, Chunk, Op};

/// Mark a run of `count` argument registers starting at `base`.
fn mark_range(mark: &mut impl FnMut(u16), base: u16, count: u16) {
    for r in base..base.saturating_add(count) {
        mark(r);
    }
}

/// Mark a handful of individually named registers.
fn mark_each<const N: usize>(mark: &mut impl FnMut(u16), regs: [u16; N]) {
    for r in regs {
        mark(r);
    }
}

/// Mark every frame register the op reads. Exhaustive on purpose: a new op
/// variant fails to compile here, which keeps the dead-store analysis
/// honest.
fn mark_reads(chunk: &Chunk, op: &Op, mark: &mut impl FnMut(u16)) {
    match op {
        Op::LoadConst { .. }
        | Op::LoadInt { .. }
        | Op::LoadIntW { .. }
        | Op::LoadBool { .. }
        | Op::LoadUnit { .. }
        | Op::LoadUpvalue { .. }
        | Op::LoadGlobal { .. }
        | Op::LoadEnum { .. }
        | Op::PathValue { .. }
        | Op::UniqueUpvalue { .. }
        | Op::Jump { .. }
        | Op::LoopHead { .. } => {}
        // `DropCell` touches no register itself, but it sends the next read
        // of the local back to the register, so the write that fills it is
        // not dead.
        Op::LoadCell { cell, .. } | Op::UniqueCell { cell, .. } | Op::DropCell { cell } => {
            mark(*cell);
        }
        Op::StoreCell { cell, src } => mark_each(mark, [*cell, *src]),
        Op::StoreUpvalue { src, .. }
        | Op::Move { src, .. }
        | Op::IterInit { src, .. }
        | Op::Deref { src, .. }
        | Op::MakeBorrow { src, .. }
        | Op::DefaultOf { src, .. }
        | Op::MoveOut { src }
        | Op::Ret { src }
        | Op::Try { src, .. }
        | Op::TryJump { src, .. }
        | Op::Cast { src, .. }
        | Op::Coerce { src, .. }
        | Op::Await { src, .. } => mark(*src),
        Op::Bin { a, b, .. } | Op::CmpJump { a, b, .. } => mark_each(mark, [*a, *b]),
        Op::BinImm { a, .. } | Op::CmpJumpImm { a, .. } | Op::Un { a, .. } => mark(*a),
        Op::JumpIfFalse { cond, .. } | Op::JumpIfTrue { cond, .. } => mark(*cond),
        Op::CallFn { base, argc, .. } | Op::CallPath { base, argc, .. } => {
            mark_range(mark, *base, *argc);
        }
        Op::CallValue {
            callee, base, argc, ..
        } => {
            mark(*callee);
            mark_range(mark, *base, *argc);
        }
        Op::Method {
            recv, base, argc, ..
        } => {
            mark(*recv);
            mark_range(mark, *base, *argc);
        }
        Op::GetOrDefault {
            recv, key, default, ..
        } => mark_each(mark, [*recv, *key, *default]),
        Op::MakeVec { base, count, .. }
        | Op::MakeTuple { base, count, .. }
        | Op::MakeEnum { base, count, .. }
        | Op::Dbg {
            base, argc: count, ..
        } => mark_range(mark, *base, *count),
        Op::MakeArrayRepeat { val, count, .. } => mark_each(mark, [*val, *count]),
        Op::MakeRange { start, end, .. } => mark_each(mark, [*start, *end]),
        Op::ForNext { iter, idx, .. } => mark_each(mark, [*iter, *idx]),
        Op::MakeStruct { info, base, .. } => mark_struct(chunk, mark, *info, *base),
        Op::MakeClosure { child, .. } | Op::Spawn { child, .. } => {
            mark_captures(chunk, mark, *child);
        }
        Op::Index { base, key, .. }
        | Op::UniqueIndex { base, key, .. }
        | Op::RefIndex { base, key, .. } => mark_each(mark, [*base, *key]),
        Op::SetIndex { base, key, val } => mark_each(mark, [*base, *key, *val]),
        Op::SetDeref { target, val }
        | Op::SetDerefParam { target, val }
        | Op::DerefBinAssign { target, val, .. } => mark_each(mark, [*target, *val]),
        Op::GetField { base, .. } | Op::UniqueField { base, .. } | Op::RefField { base, .. } => {
            mark(*base);
        }
        Op::SetField { base, val, .. } => mark_each(mark, [*base, *val]),
        Op::UniqueReg { reg } => mark(*reg),
        Op::DropScope { list } => mark_drop_list(chunk, mark, *list),
        Op::TestBind { val, .. } => mark(*val),
        Op::Fmt { spec, .. } | Op::MacroCall { spec, .. } => mark_fmt(chunk, mark, *spec),
    }
}

/// Mark the field window a struct literal builds from.
fn mark_struct(chunk: &Chunk, mark: &mut impl FnMut(u16), info: u16, base: u16) {
    let lit = &chunk.struct_lits[info as usize];
    let count = lit.shape.fields.len() + usize::from(lit.has_rest);
    mark_range(mark, base, u16::try_from(count).unwrap_or(u16::MAX));
}

/// Mark the locals a closure or task chunk captures from this frame.
fn mark_captures(chunk: &Chunk, mark: &mut impl FnMut(u16), child: u16) {
    for cap in &chunk.child_caps[child as usize] {
        if let CapSource::Local(reg) | CapSource::MutableLocal(reg) = cap {
            mark(*reg);
        }
    }
}

/// Mark the bindings a scope end runs `Drop` on.
fn mark_drop_list(chunk: &Chunk, mark: &mut impl FnMut(u16), list: u16) {
    for reg in chunk.drop_lists[list as usize].iter() {
        mark(*reg);
    }
}

/// Mark the arguments a format spec reads, positional and named alike.
fn mark_fmt(chunk: &Chunk, mark: &mut impl FnMut(u16), spec: u16) {
    let fmt = &chunk.fmts[spec as usize];
    for reg in fmt
        .positional
        .iter()
        .chain(fmt.named.iter().map(|(_, r)| r))
    {
        mark(*reg);
    }
}

/// Which frame registers any op of the chunk ever reads.
pub(super) fn chunk_reads(chunk: &Chunk) -> Vec<bool> {
    let mut read = vec![false; chunk.num_regs.max(chunk.num_params)];
    for op in &chunk.code {
        mark_reads(chunk, op, &mut |r| {
            if let Some(slot) = read.get_mut(usize::from(r)) {
                *slot = true;
            }
        });
    }
    read
}
