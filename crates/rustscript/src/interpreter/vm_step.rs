//! One dispatch step. `step` executes the op at `ctx.ip` and returns the `Flow` the frame loop in
//! `vm.rs` applies.

use std::collections::hash_map::Entry;

use rustc_hash::FxHashMap;
use std::mem::{forget, replace, take};
use std::slice::from_ref;
use std::sync::Arc;

use anyhow::{Result, bail};
use parking_lot::Mutex;

use super::bytecode::{Chunk, DefaultIr, Op};
use super::numeric::IntWidth;
use super::ops::{self, apply_bin, apply_bin_imm, apply_un, cmp_test, cmp_test_imm};
use super::value::{ClosureData, Upvalue, Value};
use super::vm::{TypeEnv, Vm};
use super::vm_method::{get_or_default, method_op};

/// Two words, so every op returns it in registers. The value of a `Ret` and the request of a
/// `Call` wait in the context.
pub(super) enum Flow {
    Next,
    Jump(usize),
    Ret,
    Call,
}

pub(super) struct CallReq {
    pub chunk: Arc<Chunk>,
    pub closure: Option<Arc<ClosureData>>,
    pub dst: u16,
    pub abase: usize,
    pub argc: usize,
    pub type_env: TypeEnv,
    /// see `Op::DropParams`
    pub owned_args: bool,
}

pub(super) struct StepCtx<'a> {
    pub vm: &'a Arc<Vm>,
    pub cur: &'a Arc<Chunk>,
    pub cur_clo: &'a Option<Arc<ClosureData>>,
    pub cur_tenv: &'a TypeEnv,
    pub entry_upvalues: &'a [Upvalue],
    pub local_cells: &'a mut FxHashMap<usize, Arc<Mutex<Value>>>,
    pub stack: &'a mut Vec<Value>,
    pub base: usize,
    pub ip: usize,
    /// what `Flow::Ret` returns
    pub ret: Value,
    /// what `Flow::Call` enters
    pub call: Option<CallReq>,
    /// whether the frame owns its by value parameters, see `Op::DropParams`
    pub owned_args: bool,
}

impl StepCtx<'_> {
    #[inline]
    pub(super) fn get(&self, reg: u16) -> &Value {
        &self.stack[self.base + reg as usize]
    }

    pub(super) fn take(&mut self, reg: u16) -> Value {
        take(&mut self.stack[self.base + reg as usize])
    }

    #[inline]
    pub(super) fn put(&mut self, reg: u16, v: Value) {
        let old = replace(&mut self.stack[self.base + reg as usize], v);
        // the drop glue of `Value` is an out of line call, a plain old value has nothing to drop
        if old.is_plain() {
            forget(old);
        }
    }

    #[inline]
    pub(super) fn set(&mut self, reg: u16, v: Value) -> Flow {
        self.put(reg, v);
        Flow::Next
    }

    pub(super) fn set_opt(&mut self, reg: u16, v: Value) -> Flow {
        if reg != u16::MAX {
            self.put(reg, v);
        }
        Flow::Next
    }

    pub(super) fn upvalues(&self) -> &[Upvalue] {
        match self.cur_clo {
            Some(c) => &c.captured,
            None => self.entry_upvalues,
        }
    }

    /// Built on first use, so a branch that never runs leaves no cell behind.
    pub(super) fn cell(&mut self, reg: u16) -> &Arc<Mutex<Value>> {
        let slot = self.base + reg as usize;
        match self.local_cells.entry(slot) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => {
                let value = self.stack[slot].clone();
                e.insert(Arc::new(Mutex::new(value)))
            }
        }
    }

    /// `first` is relative to `base`
    pub(super) fn take_range(&mut self, first: usize, count: usize) -> Vec<Value> {
        let s = self.base + first;
        (0..count).map(|i| take(&mut self.stack[s + i])).collect()
    }
}

pub(super) fn step(ctx: &mut StepCtx, op: &Op) -> Result<Flow> {
    Ok(match op {
        Op::LoadConst { .. }
        | Op::LoadInt { .. }
        | Op::LoadIntW { .. }
        | Op::LoadBool { .. }
        | Op::LoadUnit { .. }
        | Op::LoadGlobal { .. }
        | Op::LoadUpvalue { .. }
        | Op::LoadCell { .. }
        | Op::StoreCell { .. }
        | Op::DropCell { .. }
        | Op::StoreUpvalue { .. }
        | Op::Move { .. } => load_step(ctx, op)?,
        Op::Bin { .. }
        | Op::BinImm { .. }
        | Op::Un { .. }
        | Op::BinInt { .. }
        | Op::BinIntImm { .. }
        | Op::BinFloat { .. }
        | Op::CmpJumpInt { .. }
        | Op::CmpJumpIntImm { .. } => arith_step(ctx, op)?,
        Op::Jump { to } => jump(ctx, *to as usize)?,
        Op::JumpIfFalse { cond, to } => branch(!ctx.get(*cond).is_truthy(), *to),
        Op::JumpIfTrue { cond, to } => branch(ctx.get(*cond).is_truthy(), *to),
        Op::CmpJump { a, b, op, to } => branch(!cmp_test(*op, ctx.get(*a), ctx.get(*b))?, *to),
        Op::CmpJumpImm { a, imm, op, to } => branch(!cmp_test_imm(*op, ctx.get(*a), *imm)?, *to),
        Op::CallFn { .. } | Op::CallValue { .. } | Op::CallPath { .. } => call_step(ctx, op)?,
        Op::PathValue { dst, path } => path_value(ctx, *dst, *path)?,
        Op::Method {
            dst,
            recv,
            name,
            base,
            argc,
        } => method_op(ctx, *dst, *recv, *name, *base, *argc)?,
        Op::GetOrDefault {
            dst,
            recv,
            key,
            default,
        } => get_or_default(ctx, *dst, *recv, *key, *default)?,
        Op::Ret { src } => {
            ctx.ret = ctx.take(*src);
            Flow::Ret
        }
        Op::MakeVec { .. }
        | Op::MakeMap { .. }
        | Op::MakeTuple { .. }
        | Op::MakeArrayRepeat { .. }
        | Op::MakeRange { .. }
        | Op::IterInit { .. }
        | Op::ForNext { .. }
        | Op::MakeStruct { .. }
        | Op::MakeEnum { .. }
        | Op::LoadEnum { .. }
        | Op::MakeClosure { .. } => build_step(ctx, op)?,
        Op::Index { .. }
        | Op::SetIndex { .. }
        | Op::Deref { .. }
        | Op::SetDeref { .. }
        | Op::SetDerefParam { .. }
        | Op::GetField { .. }
        | Op::TakeField { .. }
        | Op::SetField { .. } => access_step(ctx, op)?,
        Op::DerefBinAssign { target, val, op } => deref_bin_assign(ctx, *target, *val, *op)?,
        Op::Take { dst, src } => {
            let v = ctx.take(*src);
            ctx.set(*dst, v)
        }
        Op::Copy { dst, src } => {
            let v = ctx.get(*src).deep_clone();
            ctx.set(*dst, v)
        }
        Op::Own { .. } => unreachable!("`Own` is resolved by the liveness pass"),
        Op::RefIndex { .. }
        | Op::RefField { .. }
        | Op::DropScope { .. }
        | Op::DropParams { .. }
        | Op::DefaultOf { .. }
        | Op::BuildDefault { .. }
        | Op::MakeBorrow { .. } => place_step(ctx, op)?,
        Op::Try { dst, src, conv } => try_op(ctx, *dst, *src, *conv)?,
        Op::TryJump { dst, src, to, conv } => try_jump(ctx, *dst, *src, *to, *conv)?,
        Op::Cast { dst, src, ty } => cast_op(ctx, *dst, *src, *ty)?,
        Op::Coerce { dst, src, ty } => coerce_op(ctx, *dst, *src, *ty),
        Op::TestBind { val, pat, dst } => test_bind(ctx, *val, *pat, *dst),
        Op::Fmt { dst, spec } => fmt_op(ctx, *dst, *spec)?,
        Op::MacroCall { kind, dst, spec } => macro_call(ctx, *kind, *dst, *spec)?,
        Op::Dbg { dst, base, argc } => dbg_op(ctx, *dst, *base, *argc),
        Op::Spawn { dst, child } => spawn_op(ctx, *dst, *child),
        Op::Await { dst, src } => await_op(ctx, *dst, *src)?,
    })
}

/// Reverse declaration order. A moved binding was cleared by its `Take`, so it holds `Unit` here.
fn drop_scope(ctx: &mut StepCtx, list: u16) -> Result<()> {
    let regs = ctx.cur.drop_lists[list as usize].clone();
    for reg in regs.iter().rev() {
        let value = ctx.take(*reg);
        ctx.vm.run_user_drop(value)?;
    }
    Ok(())
}

/// `a + b` dispatches to the user `add`. `a += b` is lowered to `a = a + b` and falls back to a
/// user `add_assign` that returns the mutated value.
fn user_bin(
    ctx: &StepCtx,
    op: super::bytecode::BinKind,
    a: &Value,
    b: &Value,
) -> Result<Option<Value>> {
    if ctx.vm.impls.is_empty() {
        return Ok(None);
    }
    let Some(methods) = ctx
        .vm
        .impls
        .of_value(a)
        .or_else(|| ctx.vm.impls.of_value(b))
    else {
        return Ok(None);
    };
    // equality and ordering go through `eq_value` and `partial_compare`
    if let Some(chunk) = methods.bin(op) {
        let chunk = chunk.clone();
        return Ok(Some(ctx.vm.run_chunk(
            &chunk,
            &[a.clone(), b.clone()],
            &[],
            false,
        )?));
    }
    if let Some(chunk) = methods.bin_assign(op) {
        let chunk = chunk.clone();
        // the mutated receiver is the store back result of `a = a + b`
        ctx.vm
            .run_chunk(&chunk, &[a.clone(), b.clone()], &[], false)?;
        return Ok(Some(a.clone()));
    }
    Ok(None)
}

/// `impl Neg for X`
fn user_un(ctx: &StepCtx, op: super::bytecode::UnKind, a: &Value) -> Result<Option<Value>> {
    if ctx.vm.impls.is_empty() {
        return Ok(None);
    }
    let Some(chunk) = ctx.vm.impls.of_value(a).and_then(|methods| methods.un(op)) else {
        return Ok(None);
    };
    let chunk = chunk.clone();
    Ok(Some(ctx.vm.run_chunk(&chunk, from_ref(a), &[], false)?))
}

/// Split out of `step` to keep the dispatch match readable.
fn build_step(ctx: &mut StepCtx, op: &Op) -> Result<Flow> {
    Ok(match op {
        Op::MakeVec { dst, base, count } => make_vec(ctx, *dst, *base, *count),
        Op::MakeMap { dst, set } => ctx.set(*dst, if *set { Value::set() } else { Value::map() }),
        Op::MakeTuple { dst, base, count } => make_tuple(ctx, *dst, *base, *count),
        Op::MakeArrayRepeat { dst, val, count } => array_repeat(ctx, *dst, *val, *count)?,
        Op::MakeRange {
            dst,
            start,
            end,
            inclusive,
        } => make_range(ctx, *dst, *start, *end, *inclusive)?,
        Op::IterInit { dst, src, owned } => {
            let source = if *owned {
                ctx.take(*src)
            } else {
                ctx.get(*src).clone()
            };
            ctx.set(*dst, ctx.vm.loop_iterator(source, *owned)?)
        }
        Op::ForNext { iter, idx, val, to } => for_next(ctx, *iter, *idx, *val, *to)?,
        Op::MakeStruct { dst, info, base } => make_struct(ctx, *dst, *info, *base),
        Op::MakeEnum {
            dst,
            info,
            base,
            count,
        } => make_enum(ctx, *dst, *info, *base, *count),
        Op::LoadEnum { dst, info } => load_enum(ctx, *dst, *info),
        Op::MakeClosure { dst, child } => closure_op(ctx, *dst, *child),
        _ => unreachable!("build_step handles only the constructor and loop ops"),
    })
}

/// Split out of `step` to keep the dispatch match readable.
fn load_step(ctx: &mut StepCtx, op: &Op) -> Result<Flow> {
    Ok(match op {
        Op::LoadConst { dst, k } => ctx.set(*dst, Value::from_const(&ctx.cur.consts[*k as usize])),
        Op::LoadInt { dst, v } => ctx.set(*dst, Value::Int(*v)),
        Op::LoadIntW { dst, v, w } => ctx.set(*dst, Value::IntW(*v, *w)),
        Op::LoadBool { dst, v } => ctx.set(*dst, Value::Bool(*v)),
        Op::LoadUnit { dst } => ctx.set(*dst, Value::Unit),
        Op::LoadGlobal { dst, idx } => ctx.set(*dst, ctx.vm.global(*idx as usize)?),
        Op::LoadUpvalue { dst, idx } => ctx.set(*dst, ctx.upvalues()[*idx as usize].get()),
        Op::LoadCell { dst, cell } => load_cell(ctx, *dst, *cell),
        Op::StoreCell { cell, src } => store_cell(ctx, *cell, *src)?,
        Op::DropCell { cell } => drop_cell(ctx, *cell),
        Op::StoreUpvalue { idx, src } => store_upvalue(ctx, *idx, *src)?,
        Op::Move { dst, src } => ctx.set(*dst, ctx.get(*src).clone()),
        _ => unreachable!("load_step handles only the load and store ops"),
    })
}

/// Split out of `step` to keep the dispatch match readable.
fn arith_step(ctx: &mut StepCtx, op: &Op) -> Result<Flow> {
    Ok(match op {
        Op::Bin { dst, a, b, op } => bin_op(ctx, *dst, *a, *b, *op)?,
        Op::BinImm { dst, a, imm, op } => bin_imm_op(ctx, *dst, *a, *imm, *op)?,
        Op::Un { dst, a, op } => un_op(ctx, *dst, *a, *op)?,
        Op::BinInt { dst, a, b, op, w } => bin_int(ctx, *dst, *a, *b, *op, *w)?,
        Op::BinIntImm { dst, a, imm, op, w } => bin_int_imm(ctx, *dst, *a, *imm, *op, *w)?,
        Op::BinFloat { dst, a, b, op, f32 } => bin_float(ctx, *dst, *a, *b, *op, *f32)?,
        Op::CmpJumpInt { a, b, op, w, to } => {
            branch(!cmp_int(ctx.get(*a), ctx.get(*b), *op, *w)?, *to)
        }
        Op::CmpJumpIntImm { a, imm, op, w, to } => {
            branch(!cmp_int(ctx.get(*a), &Value::Int(*imm), *op, *w)?, *to)
        }
        _ => unreachable!("arith_step handles only the arithmetic ops"),
    })
}

/// Split out of `step` to keep the dispatch match readable.
fn call_step(ctx: &mut StepCtx, op: &Op) -> Result<Flow> {
    match op {
        Op::CallFn {
            dst,
            func,
            base,
            argc,
            targ,
        } => call_fn(ctx, *dst, *func, *base, *argc, *targ),
        Op::CallValue {
            dst,
            callee,
            base,
            argc,
        } => call_value(ctx, *dst, *callee, *base, *argc),
        Op::CallPath {
            dst,
            path,
            base,
            argc,
        } => call_path(ctx, *dst, *path, *base, *argc),
        _ => unreachable!("call_step handles only the call ops"),
    }
}

/// Split out of `step` to keep the dispatch match readable.
fn access_step(ctx: &mut StepCtx, op: &Op) -> Result<Flow> {
    match op {
        Op::Index { dst, base, key } => index_op(ctx, *dst, *base, *key),
        Op::SetIndex { base, key, val } => set_index(ctx, *base, *key, *val),
        Op::Deref { dst, src } => deref_op(ctx, *dst, *src),
        Op::SetDeref { target, val } => set_deref(ctx, *target, *val),
        Op::SetDerefParam { target, val } => set_deref_param(ctx, *target, *val),
        Op::GetField { dst, base, member } => get_field_op(ctx, *dst, *base, *member),
        Op::SetField { base, member, val } => set_field_op(ctx, *base, *member, *val),
        Op::TakeField { dst, base, member } => take_field_op(ctx, *dst, *base, *member),
        _ => unreachable!("access_step handles only the access ops"),
    }
}

/// Split out of `step` to keep the dispatch match readable.
fn place_step(ctx: &mut StepCtx, op: &Op) -> Result<Flow> {
    Ok(match op {
        Op::RefIndex { dst, base, key } => ref_index(ctx, *dst, *base, *key)?,
        Op::RefField { dst, base, member } => ref_field(ctx, *dst, *base, *member)?,
        Op::DropScope { list } => {
            drop_scope(ctx, *list)?;
            Flow::Next
        }
        Op::DropParams { list } => {
            if ctx.owned_args {
                drop_scope(ctx, *list)?;
            }
            Flow::Next
        }
        Op::DefaultOf { dst, src } => default_of(ctx, *dst, *src),
        Op::BuildDefault { dst, ir } => {
            let v = build_default(&ctx.cur.defaults[*ir as usize]);
            ctx.set(*dst, v)
        }
        Op::MakeBorrow { dst, src } => make_borrow(ctx, *dst, *src),
        _ => unreachable!("place_step handles only the place ops"),
    })
}

fn bin_op(
    ctx: &mut StepCtx,
    dst: u16,
    a: u16,
    b: u16,
    op: super::bytecode::BinKind,
) -> Result<Flow> {
    if let Some(v) = user_bin(ctx, op, ctx.get(a), ctx.get(b))? {
        Ok(ctx.set(dst, v))
    } else {
        Ok(ctx.set(dst, apply_bin(op, ctx.get(a), ctx.get(b))?))
    }
}

fn bin_imm_op(
    ctx: &mut StepCtx,
    dst: u16,
    a: u16,
    imm: i64,
    op: super::bytecode::BinKind,
) -> Result<Flow> {
    if let Some(v) = user_bin(ctx, op, ctx.get(a), &Value::Int(imm))? {
        Ok(ctx.set(dst, v))
    } else {
        Ok(ctx.set(dst, apply_bin_imm(op, ctx.get(a), imm)?))
    }
}

/// The pass said both sides are `w`. When the values agree the width runs natively, otherwise
/// the generic op decides, so a wrong guess costs time and never correctness.
#[inline]
fn bin_int(
    ctx: &mut StepCtx,
    dst: u16,
    a: u16,
    b: u16,
    op: super::bytecode::BinKind,
    w: IntWidth,
) -> Result<Flow> {
    match ops::typed_int(op, w, ctx.get(a), ctx.get(b)) {
        Some(v) => Ok(ctx.set(dst, v?)),
        None => bin_op(ctx, dst, a, b, op),
    }
}

#[inline]
fn bin_int_imm(
    ctx: &mut StepCtx,
    dst: u16,
    a: u16,
    imm: i64,
    op: super::bytecode::BinKind,
    w: IntWidth,
) -> Result<Flow> {
    match ops::typed_int(op, w, ctx.get(a), &Value::Int(imm)) {
        Some(v) => Ok(ctx.set(dst, v?)),
        None => bin_imm_op(ctx, dst, a, imm, op),
    }
}

fn bin_float(
    ctx: &mut StepCtx,
    dst: u16,
    a: u16,
    b: u16,
    op: super::bytecode::BinKind,
    f32: bool,
) -> Result<Flow> {
    match ops::typed_float(op, f32, ctx.get(a), ctx.get(b)) {
        Some(v) => Ok(ctx.set(dst, v)),
        None => bin_op(ctx, dst, a, b, op),
    }
}

#[inline]
fn cmp_int(l: &Value, r: &Value, op: super::bytecode::BinKind, w: IntWidth) -> Result<bool> {
    match ops::typed_cmp(op, w, l, r) {
        Some(hit) => Ok(hit),
        None => cmp_test(op, l, r),
    }
}

fn un_op(ctx: &mut StepCtx, dst: u16, a: u16, op: super::bytecode::UnKind) -> Result<Flow> {
    if let Some(v) = user_un(ctx, op, ctx.get(a))? {
        Ok(ctx.set(dst, v))
    } else {
        Ok(ctx.set(dst, apply_un(op, ctx.get(a))?))
    }
}

pub(super) fn build_default(ir: &DefaultIr) -> Value {
    match ir {
        DefaultIr::Int(width) => Value::int_of_width(0, *width),
        DefaultIr::F32 => Value::F32(0.0),
        DefaultIr::F64 => Value::Float(0.0),
        DefaultIr::Bool => Value::Bool(false),
        DefaultIr::Char => Value::Char('\0'),
        DefaultIr::Str => Value::str(String::new()),
        DefaultIr::Unit => Value::Unit,
        DefaultIr::Vec => Value::vec(Vec::new()),
        DefaultIr::Map => Value::map(),
        DefaultIr::Set => Value::set(),
        DefaultIr::Opt => Value::none(),
        DefaultIr::Tuple(items) => Value::tuple(items.iter().map(build_default).collect()),
        DefaultIr::Struct { shape, fields } => {
            Value::structure(shape.clone(), fields.iter().map(build_default).collect())
        }
        DefaultIr::Enum(variant) => Value::Enum {
            def: variant.def.clone(),
            variant: variant.variant,
            data: Arc::new(Mutex::new(Vec::new())),
        },
    }
}

/// See `Value::default_like`.
fn default_of(ctx: &mut StepCtx, dst: u16, src: u16) -> Flow {
    let v = ctx.get(src).default_like();
    ctx.set(dst, v)
}

/// A value that already is a reference stays one.
fn make_borrow(ctx: &mut StepCtx, dst: u16, src: u16) -> Flow {
    let v = ctx.get(src).clone();
    let wrapped = match v {
        already @ Value::Ref(_) => already,
        plain => Value::Ref(Arc::new(super::value::ValueRef::borrowed(plain))),
    };
    ctx.set(dst, wrapped)
}

fn index_op(ctx: &mut StepCtx, dst: u16, base: u16, key: u16) -> Result<Flow> {
    let target = place_base(ctx.get(base))?;
    Ok(ctx.set(dst, ops::index(&target, ctx.get(key))?))
}

#[inline]
fn branch(jump: bool, to: u32) -> Flow {
    if jump {
        Flow::Jump(to as usize)
    } else {
        Flow::Next
    }
}

/// A backward jump runs a pending Ctrl-C handler.
fn jump(ctx: &mut StepCtx, to: usize) -> Result<Flow> {
    if to <= ctx.ip {
        ctx.vm.run_pending_ctrlc()?;
    }
    Ok(Flow::Jump(to))
}

fn load_cell(ctx: &mut StepCtx, dst: u16, cell: u16) -> Flow {
    let v = ctx.cell(cell).lock().clone();
    ctx.set(dst, v)
}

fn store_cell(ctx: &mut StepCtx, cell: u16, src: u16) -> Result<Flow> {
    let v = ctx.get(src).clone();
    let old = std::mem::replace(&mut *ctx.cell(cell).lock(), v);
    if ctx.vm.has_drop {
        ctx.vm.run_user_drop(old)?;
    }
    Ok(Flow::Next)
}

/// A binding starts a new variable, so the last cell is forgotten and the next use builds one
/// from the register.
fn drop_cell(ctx: &mut StepCtx, cell: u16) -> Flow {
    ctx.local_cells.remove(&(ctx.base + cell as usize));
    Flow::Next
}

fn store_upvalue(ctx: &StepCtx, idx: u16, src: u16) -> Result<Flow> {
    let Some(old) = ctx.upvalues()[idx as usize].swap(ctx.get(src).clone()) else {
        bail!("cannot assign to immutable capture");
    };
    if ctx.vm.has_drop {
        ctx.vm.run_user_drop(old)?;
    }
    Ok(Flow::Next)
}

mod calls;
mod control;
mod places;

use calls::{
    array_repeat, call_fn, call_path, call_value, closure_op, for_next, load_enum, make_enum,
    make_range, make_struct, make_tuple, make_vec, path_value, spawn_op,
};
use control::{
    await_op, cast_op, coerce_op, dbg_op, fmt_op, macro_call, test_bind, try_jump, try_op,
};
use places::{
    deref_bin_assign, deref_op, get_field_op, place_base, ref_field, ref_index, set_deref,
    set_deref_param, set_field_op, set_index, take_field_op,
};
