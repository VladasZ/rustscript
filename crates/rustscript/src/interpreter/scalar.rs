//! Scalar closure specialization for comparator sorts.
//!
//! A comparator sort calls its closure once per comparison, and the generic
//! call path pays a register stack, boxed values, and bridge dispatch every
//! time. A closure whose body only moves plain integers does not need any of
//! that. This module translates such a body once per sort into a small plan
//! over flat `i64` registers and runs the whole sort on unboxed elements.
//!
//! The subset is strict on purpose. Any op outside it, any register whose
//! type cannot be pinned to int, bool, or `Ordering`, any capture that is
//! not a plain int, or any arithmetic failure at run time makes the caller
//! fall back to the generic path, so semantics never change, overflow and
//! division errors included.

use std::cmp::Ordering;

use super::bytecode::{BinKind, Op};
use super::numeric::i64_arith;
use super::value::{ClosureData, Upvalue, Value};
use super::vm::Vm;

/// One op of the specialized plan. Jump targets are op indices, and the
/// translation is one plan op per bytecode op, so targets carry over as is.
enum SOp {
    /// An int constant: a literal, or a captured int snapshotted at build.
    Load {
        dst: u16,
        v: i64,
    },
    /// An `Ordering` constant as -1, 0, or 1.
    LoadOrd {
        dst: u16,
        v: i64,
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
    Jump {
        to: u32,
    },
    JumpIfFalse {
        cond: u16,
        to: u32,
    },
    JumpIfTrue {
        cond: u16,
        to: u32,
    },
    CmpJump {
        a: u16,
        b: u16,
        op: BinKind,
        to: u32,
    },
    CmpJumpImm {
        a: u16,
        imm: i64,
        op: BinKind,
        to: u32,
    },
    /// `recv.cmp(arg)` as -1, 0, or 1.
    Cmp {
        dst: u16,
        recv: u16,
        arg: u16,
    },
    Ret {
        src: u16,
    },
}

/// What a register holds, for the build-time type check. The analysis is
/// flow insensitive: a register used with two different kinds anywhere is
/// `Mixed` and rejects the plan.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Unset,
    Int,
    Bool,
    Ord,
    Mixed,
}

fn join(a: Kind, b: Kind) -> Kind {
    match (a, b) {
        (Kind::Unset, k) | (k, Kind::Unset) => k,
        (a, b) if a == b => a,
        _ => Kind::Mixed,
    }
}

fn is_arith(op: BinKind) -> bool {
    matches!(
        op,
        BinKind::Add
            | BinKind::Sub
            | BinKind::Mul
            | BinKind::Div
            | BinKind::Rem
            | BinKind::BitAnd
            | BinKind::BitOr
            | BinKind::BitXor
    )
}

fn is_compare(op: BinKind) -> bool {
    matches!(
        op,
        BinKind::Eq | BinKind::Ne | BinKind::Lt | BinKind::Le | BinKind::Gt | BinKind::Ge
    )
}

/// The `Ordering` variant a path or enum literal names, as -1, 0, or 1.
fn ordering_const(enum_name: &str, variant: &str) -> Option<i64> {
    if enum_name != "Ordering" {
        return None;
    }
    match variant {
        "Less" => Some(-1),
        "Equal" => Some(0),
        "Greater" => Some(1),
        _ => None,
    }
}

pub(super) struct ScalarPlan {
    ops: Vec<SOp>,
    num_regs: usize,
}

/// Translate one bytecode op, or answer None when it falls outside the
/// subset.
fn translate_op(clo: &ClosureData, num_regs: usize, op: &Op) -> Option<SOp> {
    let chunk = &clo.chunk;
    let reg = |r: u16| (usize::from(r) < num_regs).then_some(r);
    Some(match op {
        Op::LoadInt { dst, v } => SOp::Load {
            dst: reg(*dst)?,
            v: *v,
        },
        Op::LoadUpvalue { dst, idx } => {
            // Only an immutable plain int capture is a safe constant. A
            // mutable cell could be shared with another task, and the
            // generic path would read it per call.
            let Some(Upvalue::Value(Value::Int(v))) = clo.captured.get(*idx as usize) else {
                return None;
            };
            SOp::Load {
                dst: reg(*dst)?,
                v: *v,
            }
        }
        Op::Move { dst, src } => SOp::Move {
            dst: reg(*dst)?,
            src: reg(*src)?,
        },
        Op::Bin { dst, a, b, op } if is_arith(*op) || is_compare(*op) => SOp::Bin {
            dst: reg(*dst)?,
            a: reg(*a)?,
            b: reg(*b)?,
            op: *op,
        },
        Op::BinImm { dst, a, imm, op } if is_arith(*op) || is_compare(*op) => SOp::BinImm {
            dst: reg(*dst)?,
            a: reg(*a)?,
            imm: *imm,
            op: *op,
        },
        Op::Jump { to } => SOp::Jump { to: *to },
        Op::JumpIfFalse { cond, to } => SOp::JumpIfFalse {
            cond: reg(*cond)?,
            to: *to,
        },
        Op::JumpIfTrue { cond, to } => SOp::JumpIfTrue {
            cond: reg(*cond)?,
            to: *to,
        },
        Op::CmpJump { a, b, op, to } if is_compare(*op) => SOp::CmpJump {
            a: reg(*a)?,
            b: reg(*b)?,
            op: *op,
            to: *to,
        },
        Op::CmpJumpImm { a, imm, op, to } if is_compare(*op) => SOp::CmpJumpImm {
            a: reg(*a)?,
            imm: *imm,
            op: *op,
            to: *to,
        },
        Op::Method {
            dst,
            recv,
            name,
            base,
            argc,
        } if chunk.names[*name as usize].text == "cmp" && *argc == 1 => SOp::Cmp {
            // A discarded destination stays `u16::MAX` and the eval skips
            // the store, matching `set_opt`.
            dst: if *dst == u16::MAX { *dst } else { reg(*dst)? },
            recv: reg(*recv)?,
            arg: reg(*base)?,
        },
        Op::LoadEnum { dst, info } => {
            let variant = &chunk.enum_variants[*info as usize];
            let v = ordering_const(&variant.enum_name, &variant.variant)?;
            SOp::LoadOrd { dst: reg(*dst)?, v }
        }
        Op::PathValue { dst, path } => {
            let (segs, _) = &chunk.paths[*path as usize];
            let [.., enum_name, variant] = segs.as_slice() else {
                return None;
            };
            SOp::LoadOrd {
                dst: reg(*dst)?,
                v: ordering_const(enum_name, variant)?,
            }
        }
        Op::Ret { src } => SOp::Ret { src: reg(*src)? },
        _ => return None,
    })
}

impl ScalarPlan {
    /// Translate a two-parameter closure into a comparator plan, or answer
    /// None when any op, capture, or register type falls outside the subset.
    pub(super) fn comparator(vm: &Vm, clo: &ClosureData) -> Option<ScalarPlan> {
        let chunk = &clo.chunk;
        if chunk.num_params != 2 {
            return None;
        }
        // A script enum named `Ordering` would shadow the builtin constants
        // this plan folds, so its presence rejects the plan outright.
        if vm
            .enums
            .iter()
            .any(|def| super::resolver::bare(&def.name) == "Ordering")
        {
            return None;
        }
        let num_regs = chunk.num_regs.max(chunk.num_params);
        let ops = chunk
            .code
            .iter()
            .map(|op| translate_op(clo, num_regs, op))
            .collect::<Option<Vec<_>>>()?;
        let plan = ScalarPlan { ops, num_regs };
        plan.check_kinds(chunk.num_params)?;
        Some(plan)
    }

    /// The build-time type check: infer every register's kind to a fixpoint,
    /// then require every use to match. This is what keeps a bool from
    /// entering arithmetic and guarantees the returned value is an Ordering.
    fn check_kinds(&self, num_params: usize) -> Option<()> {
        let mut kinds = vec![Kind::Unset; self.num_regs];
        kinds[..num_params].fill(Kind::Int);
        let mut changed = true;
        while changed {
            changed = false;
            let mut set = |kinds: &mut Vec<Kind>, dst: u16, k: Kind| {
                let slot = &mut kinds[usize::from(dst)];
                let joined = join(*slot, k);
                if *slot != joined {
                    *slot = joined;
                    changed = true;
                }
            };
            for op in &self.ops {
                match op {
                    SOp::Load { dst, .. } => set(&mut kinds, *dst, Kind::Int),
                    SOp::LoadOrd { dst, .. } => set(&mut kinds, *dst, Kind::Ord),
                    SOp::Move { dst, src } => {
                        let k = kinds[usize::from(*src)];
                        set(&mut kinds, *dst, k);
                    }
                    SOp::Bin { dst, op, .. } | SOp::BinImm { dst, op, .. } => {
                        let k = if is_compare(*op) {
                            Kind::Bool
                        } else {
                            Kind::Int
                        };
                        set(&mut kinds, *dst, k);
                    }
                    SOp::Cmp { dst, .. } if *dst != u16::MAX => set(&mut kinds, *dst, Kind::Ord),
                    _ => {}
                }
            }
        }
        let want = |r: u16, k: Kind| (kinds[usize::from(r)] == k).then_some(());
        for op in &self.ops {
            match op {
                SOp::Bin { a, b, .. } | SOp::CmpJump { a, b, .. } => {
                    want(*a, Kind::Int)?;
                    want(*b, Kind::Int)?;
                }
                SOp::BinImm { a, .. } | SOp::CmpJumpImm { a, .. } => want(*a, Kind::Int)?,
                SOp::JumpIfFalse { cond, .. } | SOp::JumpIfTrue { cond, .. } => {
                    want(*cond, Kind::Bool)?;
                }
                SOp::Cmp { recv, arg, .. } => {
                    want(*recv, Kind::Int)?;
                    want(*arg, Kind::Int)?;
                }
                SOp::Ret { src } => want(*src, Kind::Ord)?,
                SOp::Move { src, .. } => {
                    if kinds[usize::from(*src)] == Kind::Mixed {
                        return None;
                    }
                }
                SOp::Load { .. } | SOp::LoadOrd { .. } | SOp::Jump { .. } => {}
            }
        }
        Some(())
    }

    /// Run the plan on one pair. None means the pair needs the generic
    /// path: an arithmetic failure, or control flow ran off the end.
    fn eval(&self, regs: &mut [i64], a: i64, b: i64) -> Option<Ordering> {
        regs.fill(0);
        regs[0] = a;
        regs[1] = b;
        let mut ip = 0usize;
        loop {
            match self.ops.get(ip)? {
                SOp::Load { dst, v } | SOp::LoadOrd { dst, v } => regs[usize::from(*dst)] = *v,
                SOp::Move { dst, src } => regs[usize::from(*dst)] = regs[usize::from(*src)],
                SOp::Bin { dst, a, b, op } => {
                    let (x, y) = (regs[usize::from(*a)], regs[usize::from(*b)]);
                    regs[usize::from(*dst)] = scalar_bin(*op, x, y)?;
                }
                SOp::BinImm { dst, a, imm, op } => {
                    let x = regs[usize::from(*a)];
                    regs[usize::from(*dst)] = scalar_bin(*op, x, *imm)?;
                }
                SOp::Jump { to } => {
                    ip = *to as usize;
                    continue;
                }
                SOp::JumpIfFalse { cond, to } => {
                    if regs[usize::from(*cond)] == 0 {
                        ip = *to as usize;
                        continue;
                    }
                }
                SOp::JumpIfTrue { cond, to } => {
                    if regs[usize::from(*cond)] != 0 {
                        ip = *to as usize;
                        continue;
                    }
                }
                SOp::CmpJump { a, b, op, to } => {
                    let (x, y) = (regs[usize::from(*a)], regs[usize::from(*b)]);
                    if !compare_i64(*op, x, y) {
                        ip = *to as usize;
                        continue;
                    }
                }
                SOp::CmpJumpImm { a, imm, op, to } => {
                    let x = regs[usize::from(*a)];
                    if !compare_i64(*op, x, *imm) {
                        ip = *to as usize;
                        continue;
                    }
                }
                SOp::Cmp { dst, recv, arg } => {
                    let o = regs[usize::from(*recv)].cmp(&regs[usize::from(*arg)]);
                    if *dst != u16::MAX {
                        regs[usize::from(*dst)] = o as i64;
                    }
                }
                SOp::Ret { src } => {
                    return Some(match regs[usize::from(*src)] {
                        ..0 => Ordering::Less,
                        0 => Ordering::Equal,
                        1.. => Ordering::Greater,
                    });
                }
            }
            ip += 1;
        }
    }
}

fn scalar_bin(op: BinKind, x: i64, y: i64) -> Option<i64> {
    Some(match op {
        _ if is_compare(op) => i64::from(compare_i64(op, x, y)),
        BinKind::BitAnd => x & y,
        BinKind::BitOr => x | y,
        BinKind::BitXor => x ^ y,
        // The checked forms mirror `i64_arith`, so a plan that overflows or
        // divides by zero falls back and the generic path raises the error.
        _ => i64_arith(op, x, y).ok()?,
    })
}

fn compare_i64(op: BinKind, x: i64, y: i64) -> bool {
    match op {
        BinKind::Eq => x == y,
        BinKind::Ne => x != y,
        BinKind::Lt => x < y,
        BinKind::Le => x <= y,
        BinKind::Gt => x > y,
        BinKind::Ge => x >= y,
        _ => unreachable!("compare carries a comparison operator"),
    }
}

/// Sort an all-int list through a specialized comparator, or answer None so
/// the caller runs the generic path. A pair the plan cannot answer aborts
/// the whole fast sort rather than guessing.
pub(super) fn scalar_sort_by(vm: &Vm, list: &[Value], clo: &ClosureData) -> Option<Vec<Value>> {
    let plan = ScalarPlan::comparator(vm, clo)?;
    let mut ints = Vec::with_capacity(list.len());
    for v in list {
        let Value::Int(i) = v else { return None };
        ints.push(*i);
    }
    let mut regs = vec![0i64; plan.num_regs];
    let mut failed = false;
    ints.sort_by(|&a, &b| {
        if failed {
            return Ordering::Equal;
        }
        plan.eval(&mut regs, a, b).unwrap_or_else(|| {
            failed = true;
            Ordering::Equal
        })
    });
    if failed {
        return None;
    }
    Some(ints.into_iter().map(Value::Int).collect())
}
