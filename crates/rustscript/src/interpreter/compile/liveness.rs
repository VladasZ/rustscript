//! Resolves every `Own` into a move or a copy. A register that is read again after the `Own` is
//! still live, and `rustc` only accepts that for a `Copy` type, so the value is copied. A dead
//! register is moved and cleared. No types are needed, the program passed the borrow checker.
//!
//! The same pass decides which captures a `move` closure takes instead of copies.

use std::collections::HashSet;

use super::FnState;
use crate::interpreter::bytecode::{CapSource, NO_ROOT, Op, Reg};

struct Bits {
    words: Vec<u64>,
}

impl Bits {
    fn new(regs: usize) -> Bits {
        Bits {
            words: vec![0; regs.div_ceil(64)],
        }
    }

    /// `DISCARD` and the other sentinels sit past the frame, they are no register.
    fn in_frame(&self, reg: usize) -> bool {
        reg < self.words.len() * 64
    }

    fn get(&self, reg: Reg) -> bool {
        let reg = usize::from(reg);
        self.in_frame(reg) && self.words[reg / 64] & (1 << (reg % 64)) != 0
    }

    fn set(&mut self, reg: Reg) {
        let reg = usize::from(reg);
        if self.in_frame(reg) {
            self.words[reg / 64] |= 1 << (reg % 64);
        }
    }

    fn clear(&mut self, reg: Reg) {
        let reg = usize::from(reg);
        if self.in_frame(reg) {
            self.words[reg / 64] &= !(1 << (reg % 64));
        }
    }

    /// true when something changed
    fn union(&mut self, other: &Bits) -> bool {
        let mut changed = false;
        for (mine, theirs) in self.words.iter_mut().zip(&other.words) {
            let next = *mine | theirs;
            changed |= next != *mine;
            *mine = next;
        }
        changed
    }
}

fn window(reads: &mut Vec<Reg>, base: Reg, count: usize) {
    for i in 0..count {
        reads.push(base + u16::try_from(i).expect("window fits u16"));
    }
}

/// The ops with an argument window or a side table behind them. A `DropScope` is not a read, a
/// dropped register was never read by the program, it is only cleaned up.
fn table_effects(f: &FnState, op: &Op, reads: &mut Vec<Reg>, writes: &mut Vec<Reg>) -> bool {
    match op {
        Op::CallFn {
            dst, base, argc, ..
        }
        | Op::CallPath {
            dst, base, argc, ..
        } => {
            window(reads, *base, usize::from(*argc));
            writes.push(*dst);
        }
        Op::CallValue {
            dst,
            callee,
            base,
            argc,
        } => {
            reads.push(*callee);
            window(reads, *base, usize::from(*argc));
            writes.push(*dst);
        }
        Op::Method {
            dst,
            recv,
            base,
            argc,
            ..
        } => {
            reads.push(*recv);
            window(reads, *base, usize::from(*argc));
            writes.push(*dst);
        }
        Op::MakeVec { dst, base, count }
        | Op::MakeTuple { dst, base, count }
        | Op::MakeEnum {
            dst, base, count, ..
        }
        | Op::Dbg {
            dst,
            base,
            argc: count,
        } => {
            window(reads, *base, usize::from(*count));
            writes.push(*dst);
        }
        Op::MakeStruct { dst, info, base } => {
            let lit = &f.struct_lits[usize::from(*info)];
            window(
                reads,
                *base,
                lit.shape.fields.len() + usize::from(lit.has_rest),
            );
            writes.push(*dst);
        }
        Op::MakeClosure { dst, child } | Op::Spawn { dst, child } => {
            for cap in &f.child_caps[usize::from(*child)] {
                if let CapSource::Local(reg) | CapSource::MutableLocal(reg) = cap {
                    reads.push(*reg);
                }
            }
            writes.push(*dst);
        }
        Op::DropScope { list } => writes.extend(f.drop_lists[usize::from(*list)].iter()),
        Op::TestBind { val, pat, dst } => {
            reads.push(*val);
            writes.extend(f.pats[usize::from(*pat)].binds.iter().map(|(_, reg)| *reg));
            writes.push(*dst);
        }
        Op::Fmt { dst, spec } | Op::MacroCall { dst, spec, .. } => {
            let spec = &f.fmts[usize::from(*spec)];
            reads.extend(spec.positional.iter());
            reads.extend(spec.named.iter().map(|(_, reg)| *reg));
            writes.push(*dst);
        }
        _ => return false,
    }
    true
}

/// The field, element and deref ops.
fn place_effects(op: &Op, reads: &mut Vec<Reg>, writes: &mut Vec<Reg>) {
    match op {
        Op::Index { dst, base, key } | Op::RefIndex { dst, base, key } => {
            reads.push(*base);
            reads.push(*key);
            writes.push(*dst);
        }
        Op::SetIndex { base, key, val } => {
            reads.push(*base);
            reads.push(*key);
            reads.push(*val);
        }
        Op::SetDeref { target, val } | Op::DerefBinAssign { target, val, .. } => {
            reads.push(*target);
            reads.push(*val);
        }
        Op::SetDerefParam { target, val } => {
            reads.push(*val);
            writes.push(*target);
        }
        Op::GetField { dst, base, .. } | Op::RefField { dst, base, .. } => {
            reads.push(*base);
            writes.push(*dst);
        }
        Op::SetField { base, val, .. } => {
            reads.push(*base);
            reads.push(*val);
        }
        _ => unreachable!("not a place op"),
    }
}

/// The arithmetic and the compare jumps.
fn scalar_effects(op: &Op, reads: &mut Vec<Reg>, writes: &mut Vec<Reg>) {
    match op {
        Op::Take { dst, src } => {
            reads.push(*src);
            writes.push(*src);
            writes.push(*dst);
        }
        Op::Bin { dst, a, b, .. }
        | Op::BinInt { dst, a, b, .. }
        | Op::BinFloat { dst, a, b, .. } => {
            reads.push(*a);
            reads.push(*b);
            writes.push(*dst);
        }
        Op::JumpIfFalse { cond, .. } | Op::JumpIfTrue { cond, .. } => reads.push(*cond),
        Op::CmpJump { a, b, .. } | Op::CmpJumpInt { a, b, .. } => {
            reads.push(*a);
            reads.push(*b);
        }
        Op::CmpJumpImm { a, .. } | Op::CmpJumpIntImm { a, .. } => reads.push(*a),
        _ => unreachable!("not a scalar op"),
    }
}

/// `reads` and `writes` of one op.
fn effects(f: &FnState, op: &Op, reads: &mut Vec<Reg>, writes: &mut Vec<Reg>) {
    if table_effects(f, op, reads, writes) {
        return;
    }
    match op {
        Op::LoadConst { dst, .. }
        | Op::LoadInt { dst, .. }
        | Op::LoadIntW { dst, .. }
        | Op::LoadBool { dst, .. }
        | Op::LoadUnit { dst }
        | Op::LoadUpvalue { dst, .. }
        | Op::LoadGlobal { dst, .. }
        | Op::PathValue { dst, .. }
        | Op::MakeMap { dst, .. }
        | Op::LoadEnum { dst, .. }
        | Op::BuildDefault { dst, .. } => writes.push(*dst),
        Op::LoadCell { dst, cell } => {
            reads.push(*cell);
            writes.push(*dst);
        }
        Op::StoreCell { cell, src } => {
            reads.push(*src);
            writes.push(*cell);
        }
        Op::DropCell { .. } | Op::Jump { .. } => {}
        Op::StoreUpvalue { src, .. } | Op::Ret { src } => reads.push(*src),
        Op::Move { dst, src }
        | Op::Copy { dst, src }
        | Op::IterInit { dst, src, .. }
        | Op::Deref { dst, src }
        | Op::MakeBorrow { dst, src }
        | Op::DefaultOf { dst, src }
        | Op::Try { dst, src, .. }
        | Op::TryJump { dst, src, .. }
        | Op::Cast { dst, src, .. }
        | Op::Coerce { dst, src, .. }
        | Op::Await { dst, src }
        | Op::Un { dst, a: src, .. }
        | Op::BinImm { dst, a: src, .. }
        | Op::BinIntImm { dst, a: src, .. }
        | Op::Own { dst, src, .. } => {
            reads.push(*src);
            writes.push(*dst);
        }
        Op::Take { .. }
        | Op::Bin { .. }
        | Op::BinInt { .. }
        | Op::BinFloat { .. }
        | Op::JumpIfFalse { .. }
        | Op::JumpIfTrue { .. }
        | Op::CmpJump { .. }
        | Op::CmpJumpInt { .. }
        | Op::CmpJumpImm { .. }
        | Op::CmpJumpIntImm { .. } => scalar_effects(op, reads, writes),
        Op::GetOrDefault {
            dst,
            recv,
            key,
            default,
        } => {
            reads.push(*recv);
            reads.push(*key);
            reads.push(*default);
            writes.push(*dst);
        }
        Op::MakeArrayRepeat { dst, val, count } => {
            reads.push(*val);
            reads.push(*count);
            writes.push(*dst);
        }
        Op::MakeRange {
            dst, start, end, ..
        } => {
            reads.push(*start);
            reads.push(*end);
            writes.push(*dst);
        }
        Op::ForNext { iter, idx, val, .. } => {
            reads.push(*iter);
            reads.push(*idx);
            writes.push(*idx);
            writes.push(*val);
        }
        Op::Index { .. }
        | Op::RefIndex { .. }
        | Op::SetIndex { .. }
        | Op::SetDeref { .. }
        | Op::DerefBinAssign { .. }
        | Op::SetDerefParam { .. }
        | Op::GetField { .. }
        | Op::RefField { .. }
        | Op::SetField { .. } => place_effects(op, reads, writes),
        _ => unreachable!("handled by `table_effects`"),
    }
}

fn successors(op: &Op, at: usize, out: &mut Vec<usize>) {
    match op {
        Op::Jump { to } => out.push(*to as usize),
        Op::Ret { .. } => {}
        Op::JumpIfFalse { to, .. }
        | Op::JumpIfTrue { to, .. }
        | Op::CmpJump { to, .. }
        | Op::CmpJumpImm { to, .. }
        | Op::CmpJumpInt { to, .. }
        | Op::CmpJumpIntImm { to, .. }
        | Op::ForNext { to, .. }
        | Op::TryJump { to, .. } => {
            out.push(at + 1);
            out.push(*to as usize);
        }
        _ => out.push(at + 1),
    }
}

impl FnState {
    /// Every `Own` becomes a `Take` or a `Copy`, and `child_moves` is filled.
    pub(super) fn resolve_owns(&mut self) {
        let regs = usize::from(self.max_reg).max(1);
        let n = self.code.len();
        // A register a closure captured is read by every later call of that closure, so it is
        // never dead. A cell promoted local is shared the same way.
        let mut pinned: HashSet<Reg> = self.mutable_locals.iter().copied().collect();
        for caps in &self.child_caps {
            for cap in caps {
                if let CapSource::Local(reg) | CapSource::MutableLocal(reg) = cap {
                    pinned.insert(*reg);
                }
            }
        }
        let mut reads: Vec<Vec<Reg>> = Vec::with_capacity(n);
        let mut writes: Vec<Vec<Reg>> = Vec::with_capacity(n);
        let mut succ: Vec<Vec<usize>> = Vec::with_capacity(n);
        for (at, op) in self.code.iter().enumerate() {
            let (mut r, mut w, mut s) = (Vec::new(), Vec::new(), Vec::new());
            effects(self, op, &mut r, &mut w);
            successors(op, at, &mut s);
            reads.push(r);
            writes.push(w);
            succ.push(s);
        }
        // live-in per op, to a fixpoint
        let mut live_in: Vec<Bits> = (0..n).map(|_| Bits::new(regs)).collect();
        let mut changed = true;
        while changed {
            changed = false;
            for at in (0..n).rev() {
                let mut out = Bits::new(regs);
                for &s in &succ[at] {
                    if s < n {
                        out.union(&live_in[s]);
                    }
                }
                for &w in &writes[at] {
                    out.clear(w);
                }
                for &r in &reads[at] {
                    out.set(r);
                }
                changed |= live_in[at].union(&out);
            }
        }
        let live_out = |at: usize, reg: Reg| -> bool {
            pinned.contains(&reg) || succ[at].iter().any(|&s| s < n && live_in[s].get(reg))
        };
        for at in 0..n {
            let Op::Own { dst, src, root } = self.code[at] else {
                if let Op::MakeClosure { child, .. } | Op::Spawn { child, .. } = self.code[at] {
                    let child = usize::from(child);
                    let moves = self.children[child].moves;
                    let takes: Vec<bool> = self.child_caps[child]
                        .iter()
                        .map(|cap| match cap {
                            CapSource::Local(reg) | CapSource::MutableLocal(reg) => {
                                moves && !live_out(at, *reg)
                            }
                            CapSource::Upvalue(_) | CapSource::MutableUpvalue(_) => false,
                        })
                        .collect();
                    self.child_moves[child] = takes.into();
                }
                continue;
            };
            self.code[at] = if root == NO_ROOT || live_out(at, root) {
                Op::Copy { dst, src }
            } else if dst == src {
                // a field or element moved out of a dead local, the local is cleared so its
                // scope end can't drop the moved part again
                Op::LoadUnit { dst: root }
            } else {
                Op::Take { dst, src }
            };
        }
    }
}
