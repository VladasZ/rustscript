//! One dispatch step of the register machine. `step` executes the op at
//! `ctx.ip` and answers with the `Flow` the frame loop in `vm.rs` applies:
//! fall through, jump, return, or push a call frame. The frame bookkeeping
//! stays in `exec`, the op bodies live here.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::iter::repeat_n;
use std::mem::take;
use std::slice::from_ref;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use anyhow::{Result, anyhow, bail};
use num_traits::AsPrimitive;
use parking_lot::Mutex;

use super::bytecode::{CapSource, Chunk, MacroKind, Member, Op, path_call_chunk};
use super::iterator::FastNext;
use super::native::Native;
use super::numeric::{float_to_int, truncate};
use super::ops::{self, apply_bin, apply_bin_imm, apply_un, cmp_test, cmp_test_imm, int_of};
use super::pattern::{bind_pattern_refs, try_bind};
use super::typeir::CastIr;
use super::value::{ClosureData, StructShape, Upvalue, Value};
use super::vm::{TypeEnv, Vm, empty_type_env};
use super::vm_method::{get_or_default, method_op};

/// What the executed op asks the frame loop to do next.
pub(super) enum Flow {
    Next,
    Jump(usize),
    Ret(Value),
    Call(CallReq),
}

/// A call the frame loop should push: the callee and its calling convention.
pub(super) struct CallReq {
    pub chunk: Arc<Chunk>,
    pub closure: Option<Arc<ClosureData>>,
    pub dst: u16,
    pub abase: usize,
    pub argc: usize,
    pub type_env: TypeEnv,
}

/// The execution state one op step sees, borrowed from the frame loop.
pub(super) struct StepCtx<'a> {
    pub vm: &'a Arc<Vm>,
    pub cur: &'a Arc<Chunk>,
    pub cur_clo: &'a Option<Arc<ClosureData>>,
    pub cur_tenv: &'a TypeEnv,
    pub entry_upvalues: &'a [Upvalue],
    pub local_cells: &'a mut HashMap<usize, Arc<Mutex<Value>>>,
    pub stack: &'a mut Vec<Value>,
    pub base: usize,
    pub ip: usize,
    /// Call frames below this one, for the function plan's depth budget.
    pub depth: usize,
}

impl StepCtx<'_> {
    pub(super) fn get(&self, reg: u16) -> &Value {
        &self.stack[self.base + reg as usize]
    }

    pub(super) fn take(&mut self, reg: u16) -> Value {
        take(&mut self.stack[self.base + reg as usize])
    }

    pub(super) fn put(&mut self, reg: u16, v: Value) {
        self.stack[self.base + reg as usize] = v;
    }

    /// Write a register and fall through to the next op.
    pub(super) fn set(&mut self, reg: u16, v: Value) -> Flow {
        self.put(reg, v);
        Flow::Next
    }

    /// Write a register unless the compiler discarded the result.
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

    /// The cell a mutably captured local lives in, built on first use.
    /// The local keeps living in its register until some closure captures
    /// it, so a branch that never runs leaves no cell behind and the first
    /// read or write that lands here builds one from the register.
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

    /// Move a run of registers out of the frame, `first` relative to `base`.
    pub(super) fn take_range(&mut self, first: usize, count: usize) -> Vec<Value> {
        let s = self.base + first;
        (0..count).map(|i| take(&mut self.stack[s + i])).collect()
    }
}

pub(super) fn step(ctx: &mut StepCtx, op: &Op) -> Result<Flow> {
    Ok(match op {
        Op::LoadConst { dst, k } => ctx.set(*dst, Value::from_const(&ctx.cur.consts[*k as usize])),
        Op::LoadInt { dst, v } => ctx.set(*dst, Value::Int(*v)),
        Op::LoadIntW { dst, v, w } => ctx.set(*dst, Value::IntW(*v, *w)),
        Op::LoadBool { dst, v } => ctx.set(*dst, Value::Bool(*v)),
        Op::LoadUnit { dst } => ctx.set(*dst, Value::Unit),
        Op::LoadGlobal { dst, idx } => ctx.set(*dst, ctx.vm.global(*idx as usize)?),
        Op::LoadUpvalue { dst, idx } => ctx.set(*dst, ctx.upvalues()[*idx as usize].get()),
        Op::LoadCell { dst, cell } => load_cell(ctx, *dst, *cell),
        Op::StoreCell { cell, src } => store_cell(ctx, *cell, *src),
        Op::DropCell { cell } => drop_cell(ctx, *cell),
        Op::StoreUpvalue { idx, src } => store_upvalue(ctx, *idx, *src)?,
        Op::Move { dst, src } => ctx.set(*dst, ctx.get(*src).clone()),
        Op::Bin { dst, a, b, op } => bin_op(ctx, *dst, *a, *b, *op)?,
        Op::BinImm { dst, a, imm, op } => bin_imm_op(ctx, *dst, *a, *imm, *op)?,
        Op::Un { dst, a, op } => un_op(ctx, *dst, *a, *op)?,
        Op::Jump { to } => jump(ctx, *to as usize)?,
        Op::LoopHead { jump } => loop_head(ctx, *jump)?,
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
        Op::Ret { src } => Flow::Ret(ctx.take(*src)),
        Op::MakeVec { dst, base, count } => make_vec(ctx, *dst, *base, *count),
        Op::MakeTuple { dst, base, count } => make_tuple(ctx, *dst, *base, *count),
        Op::MakeArrayRepeat { dst, val, count } => array_repeat(ctx, *dst, *val, *count)?,
        Op::MakeRange {
            dst,
            start,
            end,
            inclusive,
        } => make_range(ctx, *dst, *start, *end, *inclusive)?,
        Op::IterInit { dst, src } => ctx.set(*dst, ctx.vm.iterator_value(ctx.get(*src).clone())?),
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
        Op::Index { .. }
        | Op::SetIndex { .. }
        | Op::Deref { .. }
        | Op::SetDeref { .. }
        | Op::SetDerefParam { .. }
        | Op::GetField { .. }
        | Op::SetField { .. } => access_step(ctx, op)?,
        Op::DerefBinAssign { target, val, op } => deref_bin_assign(ctx, *target, *val, *op)?,
        Op::UniqueReg { .. }
        | Op::UniqueField { .. }
        | Op::UniqueIndex { .. }
        | Op::UniqueCell { .. }
        | Op::UniqueUpvalue { .. }
        | Op::RefIndex { .. }
        | Op::RefField { .. }
        | Op::DropScope { .. }
        | Op::MoveOut { .. }
        | Op::DefaultOf { .. }
        | Op::MakeBorrow { .. } => place_step(ctx, op)?,
        Op::Try { dst, src } => try_op(ctx, *dst, *src),
        Op::TryJump { dst, src, to } => try_jump(ctx, *dst, *src, *to),
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

/// Clear a moved-out binding register when its value has a user `Drop`
/// impl, so the copy in the argument window is the last holder and the
/// guard drops at the destination, not at this scope's end.
fn move_out(ctx: &mut StepCtx, src: u16) -> Flow {
    let ty = match ctx.get(src) {
        Value::Struct(s) => s.name().to_string(),
        Value::Enum { enum_name, .. } => enum_name.to_string(),
        _ => return Flow::Next,
    };
    if ctx.vm.methods.contains_key(&(ty, "Drop::drop".to_string())) {
        ctx.put(src, Value::Unit);
    }
    Flow::Next
}

/// Run user `Drop` impls for a finished scope's bindings, in reverse
/// declaration order. A binding whose storage still has another holder was
/// moved out or is still shared, its real owner drops it later.
fn drop_scope(ctx: &mut StepCtx, list: u16) -> Result<()> {
    let regs = ctx.cur.drop_lists[list as usize].clone();
    for reg in regs.iter().rev() {
        let value = ctx.take(*reg);
        ctx.vm.run_user_drop(value)?;
    }
    Ok(())
}

/// The user type name an operator could dispatch on, for `impl Add for X`.
fn user_op_type(v: &Value) -> Option<&str> {
    match v {
        Value::Struct(s) => Some(s.name()),
        Value::Enum { enum_name, .. } => Some(enum_name),
        _ => None,
    }
}

/// A binary operator on a value whose type has the matching operator trait
/// impl. `a + b` dispatches to the user `add`, and `a += b`, which the
/// compiler lowers to `a = a + b`, falls back to a user `add_assign` that
/// mutates its receiver in place and answers the mutated value.
fn user_bin(
    ctx: &StepCtx,
    op: super::bytecode::BinKind,
    a: &Value,
    b: &Value,
) -> Result<Option<Value>> {
    use super::bytecode::BinKind as K;
    if ctx.vm.methods.is_empty() {
        return Ok(None);
    }
    let Some(ty) = user_op_type(a).or_else(|| user_op_type(b)) else {
        return Ok(None);
    };
    let name = match op {
        K::Add => "add",
        K::Sub => "sub",
        K::Mul => "mul",
        K::Div => "div",
        K::Rem => "rem",
        K::BitAnd => "bitand",
        K::BitOr => "bitor",
        K::BitXor => "bitxor",
        K::Shl => "shl",
        K::Shr => "shr",
        // Equality and ordering answer through `eq_value` and
        // `partial_compare`, whose derived semantics `apply_bin` runs.
        K::Eq | K::Ne | K::Lt | K::Le | K::Gt | K::Ge => return Ok(None),
    };
    let ty = ty.to_string();
    if let Some(chunk) = ctx.vm.methods.get(&(ty.clone(), name.to_string())) {
        let chunk = chunk.clone();
        return Ok(Some(ctx.vm.run_chunk(
            &chunk,
            &[a.clone(), b.clone()],
            &[],
        )?));
    }
    let assign = format!("{name}_assign");
    if let Some(chunk) = ctx.vm.methods.get(&(ty, assign)) {
        let chunk = chunk.clone();
        // The receiver mutates in place through its `&mut self`, and the
        // mutated value is the store-back result of the lowered `a = a + b`.
        ctx.vm.run_chunk(&chunk, &[a.clone(), b.clone()], &[])?;
        return Ok(Some(a.clone()));
    }
    Ok(None)
}

/// A unary operator with a user trait impl, `impl Neg for X`.
fn user_un(ctx: &StepCtx, op: super::bytecode::UnKind, a: &Value) -> Result<Option<Value>> {
    use super::bytecode::UnKind as U;
    if ctx.vm.methods.is_empty() {
        return Ok(None);
    }
    let Some(ty) = user_op_type(a) else {
        return Ok(None);
    };
    let name = match op {
        U::Neg => "neg",
        U::Not => "not",
    };
    let Some(chunk) = ctx.vm.methods.get(&(ty.to_string(), name.to_string())) else {
        return Ok(None);
    };
    let chunk = chunk.clone();
    Ok(Some(ctx.vm.run_chunk(&chunk, from_ref(a), &[])?))
}

/// The three call shapes, split from `step` to keep the dispatch match
/// readable.
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

/// The field, index, and dereference ops, split from `step` to keep the
/// dispatch match readable.
fn access_step(ctx: &mut StepCtx, op: &Op) -> Result<Flow> {
    match op {
        Op::Index { dst, base, key } => index_op(ctx, *dst, *base, *key),
        Op::SetIndex { base, key, val } => set_index(ctx, *base, *key, *val),
        Op::Deref { dst, src } => deref_op(ctx, *dst, *src),
        Op::SetDeref { target, val } => set_deref(ctx, *target, *val),
        Op::SetDerefParam { target, val } => set_deref_param(ctx, *target, *val),
        Op::GetField { dst, base, member } => get_field_op(ctx, *dst, *base, *member),
        Op::SetField { base, member, val } => set_field_op(ctx, *base, *member, *val),
        _ => unreachable!("access_step handles only the access ops"),
    }
}

/// The place ops: uniqueness splits, reference builders, scope drops, and
/// borrow wrapping. Split from `step` to keep the dispatch match readable.
fn place_step(ctx: &mut StepCtx, op: &Op) -> Result<Flow> {
    Ok(match op {
        Op::UniqueReg { reg } => unique_reg(ctx, *reg),
        Op::UniqueField { dst, base, member } => unique_field(ctx, *dst, *base, *member)?,
        Op::UniqueIndex { dst, base, key } => unique_index(ctx, *dst, *base, *key)?,
        Op::UniqueCell { dst, cell } => unique_cell(ctx, *dst, *cell),
        Op::UniqueUpvalue { dst, idx } => unique_upvalue(ctx, *dst, *idx),
        Op::RefIndex { dst, base, key } => ref_index(ctx, *dst, *base, *key)?,
        Op::RefField { dst, base, member } => ref_field(ctx, *dst, *base, *member)?,
        Op::DropScope { list } => {
            drop_scope(ctx, *list)?;
            Flow::Next
        }
        Op::MoveOut { src } => move_out(ctx, *src),
        Op::DefaultOf { dst, src } => default_of(ctx, *dst, *src),
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

fn un_op(ctx: &mut StepCtx, dst: u16, a: u16, op: super::bytecode::UnKind) -> Result<Flow> {
    if let Some(v) = user_un(ctx, op, ctx.get(a))? {
        Ok(ctx.set(dst, v))
    } else {
        Ok(ctx.set(dst, apply_un(op, ctx.get(a))?))
    }
}

/// A fresh value of the same shape as `src`, see `Value::default_like`.
fn default_of(ctx: &mut StepCtx, dst: u16, src: u16) -> Flow {
    let v = ctx.get(src).default_like();
    ctx.set(dst, v)
}

/// Wrap a place-loaded value as a mutable borrow of its own storage. A
/// value that already is a reference stays one.
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

fn branch(jump: bool, to: u32) -> Flow {
    if jump {
        Flow::Jump(to as usize)
    } else {
        Flow::Next
    }
}

/// A backward jump closes a loop iteration, the moment to run a pending
/// Ctrl-C handler, and the moment the scalar while plan takes over the whole
/// loop when its ops qualify, see `scalar_loop.rs`. A rejected loop's jump
/// runs per iteration, so its whole cost here is the one atomic load.
fn jump(ctx: &mut StepCtx, to: usize) -> Result<Flow> {
    if to <= ctx.ip {
        ctx.vm.run_pending_ctrlc()?;
        if ctx
            .cur
            .while_rejected
            .get(ctx.ip)
            .is_some_and(|rejected| rejected.load(Ordering::Relaxed) == 0)
            && let Some(flow) = loop_plan_jump(ctx, to)?
        {
            return Ok(flow);
        }
    }
    Ok(Flow::Jump(to))
}

/// The not-yet-rejected side of a backward jump, out of the hot path: mark a
/// `for` body's back jump rejected on first sight, the `for` plan already
/// owns that loop, or hand the loop to the while plan. Cold because it runs
/// once per loop entry, not per iteration.
#[cold]
fn loop_plan_jump(ctx: &mut StepCtx, to: usize) -> Result<Option<Flow>> {
    if matches!(ctx.cur.code.get(to), Some(Op::ForNext { .. })) {
        if let Some(rejected) = ctx.cur.while_rejected.get(ctx.ip) {
            rejected.store(1, Ordering::Relaxed);
        }
        return Ok(None);
    }
    super::scalar_while::try_run_while(ctx, to)
}

/// A `LoopHead` at a loop entry: hand the loop to the while plan before the
/// first iteration runs, or fall through into the head when the loop has no
/// plan.
fn loop_head(ctx: &mut StepCtx, jump: u32) -> Result<Flow> {
    let jump_ip = jump as usize;
    if ctx
        .cur
        .while_rejected
        .get(jump_ip)
        .is_some_and(|rejected| rejected.load(Ordering::Relaxed) == 0)
        && let Some(flow) = super::scalar_while::try_run_entry(ctx, jump_ip)?
    {
        return Ok(flow);
    }
    Ok(Flow::Next)
}

fn load_cell(ctx: &mut StepCtx, dst: u16, cell: u16) -> Flow {
    let v = ctx.cell(cell).lock().clone();
    ctx.set(dst, v)
}

fn store_cell(ctx: &mut StepCtx, cell: u16, src: u16) -> Flow {
    let v = ctx.get(src).clone();
    *ctx.cell(cell).lock() = v;
    Flow::Next
}

/// A binding of a mutably captured local starts a new variable, so the cell
/// the last one shared is forgotten here. The register still holds the new
/// value, and the next read, write, or capture builds the cell from it.
fn drop_cell(ctx: &mut StepCtx, cell: u16) -> Flow {
    ctx.local_cells.remove(&(ctx.base + cell as usize));
    Flow::Next
}

fn store_upvalue(ctx: &StepCtx, idx: u16, src: u16) -> Result<Flow> {
    if !ctx.upvalues()[idx as usize].set(ctx.get(src).clone()) {
        bail!("cannot assign to immutable capture");
    }
    Ok(Flow::Next)
}

fn call_fn(
    ctx: &mut StepCtx,
    dst: u16,
    func: u32,
    abase: u16,
    argc: u16,
    targ: u32,
) -> Result<Flow> {
    let callee = ctx.vm.functions[func as usize].clone();
    // A self-recursive scalar function runs its whole call tree unboxed
    // inside this dispatch, see `scalar_fn`.
    if targ == u32::MAX
        && let Some(v) = super::scalar_fn::try_call(ctx, &callee, abase, argc)?
    {
        return Ok(ctx.set(dst, v));
    }
    // Bind the call's turbofish type args to the callee's generic parameters.
    let type_env: TypeEnv = if targ == u32::MAX {
        empty_type_env()
    } else {
        let targs = &ctx.cur.call_type_args[targ as usize];
        callee
            .generics
            .iter()
            .zip(targs.iter())
            .map(|(name, ty)| (name.clone(), ty.clone()))
            .collect()
    };
    request_call(callee, None, dst, abase, argc, type_env)
}

fn call_value(ctx: &StepCtx, dst: u16, callee: u16, abase: u16, argc: u16) -> Result<Flow> {
    let clo = match ctx.get(callee) {
        Value::Closure(clo) => clo.clone(),
        other => bail!("cannot call {}", other.type_name()),
    };
    let chunk = clo.chunk.clone();
    request_call(chunk, Some(clo), dst, abase, argc, empty_type_env())
}

/// Validate the arg count here, where the error can name the callee, then
/// hand the frame push to the loop in `exec`.
fn request_call(
    chunk: Arc<Chunk>,
    closure: Option<Arc<ClosureData>>,
    dst: u16,
    abase: u16,
    argc: u16,
    type_env: TypeEnv,
) -> Result<Flow> {
    // A path forwarder's arity is only a guess, so rebuild it for the count
    // actually passed. `u8::saturating_add` handed to `fold` takes two
    // arguments where the guess was one.
    let chunk = if chunk.path_forwarder && argc as usize != chunk.num_params {
        path_call_chunk(chunk.paths[0].0.clone(), argc as usize)
    } else {
        chunk
    };
    if argc as usize != chunk.num_params {
        bail!(
            "`{}` expects {} args but got {}",
            chunk.name,
            chunk.num_params,
            argc
        );
    }
    Ok(Flow::Call(CallReq {
        chunk,
        closure,
        dst,
        abase: abase as usize,
        argc: argc as usize,
        type_env,
    }))
}

fn call_path(ctx: &mut StepCtx, dst: u16, path: u16, abase: u16, argc: u16) -> Result<Flow> {
    let (vm, cur) = (ctx.vm, ctx.cur);
    let (abase, argc) = (abase as usize, argc as usize);
    let (segs, coerce) = &cur.paths[path as usize];
    if let Some(v) = internal_path(segs, &ctx.stack[ctx.base..], abase, argc)? {
        return Ok(ctx.set(dst, v));
    }
    let call_args = ctx.take_range(abase, argc);
    // Typed json parses straight into the target structs, no generic tree and
    // no coercion pass afterwards.
    if let Some(ty) = coerce {
        let canon = vm.canonical(segs);
        if canon.len() >= 2
            && canon[canon.len() - 2] == "serde_json"
            && canon[canon.len() - 1] == "from_str"
        {
            return Ok(ctx.set(dst, vm.typed_from_str(&call_args, ty, ctx.cur_tenv)?));
        }
    }
    let mut v = vm.dispatch_call(segs, call_args)?;
    if let Some(ty) = coerce {
        v = vm.coerce_result(v, ty);
    }
    Ok(ctx.set(dst, v))
}

/// The compiler-internal paths, `::unreachable_match` and friends.
fn internal_path(
    segments: &[String],
    registers: &[Value],
    base: usize,
    count: usize,
) -> Result<Option<Value>> {
    let head = segments.first().map_or("", String::as_str);
    match head {
        "::unreachable_match" => bail!("no match arm matched the value"),
        "::assert_failed" => bail!("assertion failed"),
        "::ensure_fail" => {
            let message = if count > 0 {
                registers[base].display()
            } else {
                "condition failed".to_string()
            };
            Ok(Some(Value::err(Value::str(message))))
        }
        _ => Ok(None),
    }
}

fn path_value(ctx: &mut StepCtx, dst: u16, path: u16) -> Result<Flow> {
    let (segs, _) = &ctx.cur.paths[path as usize];
    Ok(ctx.set(dst, ctx.vm.eval_path_value(segs)?))
}

fn make_vec(ctx: &mut StepCtx, dst: u16, first: u16, count: u16) -> Flow {
    let items = ctx.take_range(first as usize, count as usize);
    ctx.set(dst, Value::vec(items))
}

fn make_tuple(ctx: &mut StepCtx, dst: u16, first: u16, count: u16) -> Flow {
    let items = ctx.take_range(first as usize, count as usize);
    ctx.set(dst, Value::tuple(items))
}

fn array_repeat(ctx: &mut StepCtx, dst: u16, val: u16, count: u16) -> Result<Flow> {
    let n = match ctx.get(count) {
        Value::Int(n) => usize::try_from(*n)?,
        v if v.untag_int().is_some() => usize::try_from(v.untag_int().unwrap())?,
        _ => bail!("array repeat length must be an integer"),
    };
    let v = ctx.get(val).clone();
    Ok(ctx.set(dst, Value::vec(repeat_n(v, n).collect())))
}

fn make_range(ctx: &mut StepCtx, dst: u16, start: u16, end: u16, inclusive: bool) -> Result<Flow> {
    let start = int_of(ctx.get(start))?;
    let end = int_of(ctx.get(end))?;
    Ok(ctx.set(
        dst,
        Value::Range {
            start,
            end,
            inclusive,
        },
    ))
}

fn for_next(ctx: &mut StepCtx, iter: u16, idx: u16, val: u16, to: u32) -> Result<Flow> {
    // The first iteration tries the scalar plan, which runs the whole loop
    // on unboxed values when the body is int-only bytecode over a bytes or
    // range source, see scalar_loop.rs. A fallback mid-loop leaves the index
    // register at the consumed count, so the attempt happens once and the
    // index is re-read below.
    if matches!(ctx.get(idx), Value::Int(0))
        && let Some(flow) = super::scalar_for::try_run(ctx, iter, idx, to)?
    {
        return Ok(flow);
    }
    let i = match ctx.get(idx) {
        Value::Int(i) => *i,
        _ => unreachable!("for index is an integer"),
    };
    // The simple source states produce their item in place under one lock,
    // so a tight loop skips the handle clone and the step dispatch of the
    // full `iterator_next` machinery.
    let item = {
        let Value::Native(iterator) = ctx.get(iter) else {
            bail!("{} is not an iterator", ctx.get(iter).type_name());
        };
        let fast = match &mut *iterator.lock() {
            Native::Iterator(state) => state.fast_next(),
            _ => FastNext::NotSimple,
        };
        match fast {
            FastNext::Ready(item) => item,
            FastNext::NotSimple => {
                let iterator = iterator.clone();
                ctx.vm.iterator_next(&iterator)?
            }
        }
    };
    let Some(v) = item else {
        return Ok(Flow::Jump(to as usize));
    };
    ctx.put(val, v);
    ctx.vm.run_pending_ctrlc()?;
    Ok(ctx.set(idx, Value::Int(i + 1)))
}

fn make_struct(ctx: &mut StepCtx, dst: u16, info: u16, first: u16) -> Flow {
    let lit = &ctx.cur.struct_lits[info as usize];
    let written = lit.shape.fields.len();
    let mut values = ctx.take_range(first as usize, written);
    let v = if lit.has_rest {
        let rest = ctx.stack[ctx.base + first as usize + written].clone();
        let mut fields = lit.shape.fields.clone();
        let mut renames = lit.shape.renames.clone();
        if let Value::Struct(r) = rest {
            let rvals = r.values.lock();
            for (slot, (k, v)) in r.shape.fields.iter().zip(rvals.iter()).enumerate() {
                if lit.shape.slot(k).is_none() {
                    fields.push(k.clone());
                    values.push(v.clone());
                    if !renames.is_empty() {
                        renames.push(r.shape.renames.get(slot).cloned().flatten());
                    }
                }
            }
        }
        let shape = Arc::new(StructShape {
            name: lit.shape.name.clone(),
            fields,
            renames,
        });
        Value::structure(shape, values)
    } else {
        Value::structure(lit.shape.clone(), values)
    };
    ctx.set(dst, v)
}

fn make_enum(ctx: &mut StepCtx, dst: u16, info: u16, first: u16, count: u16) -> Flow {
    let variant = &ctx.cur.enum_variants[info as usize];
    let data = Arc::new(Mutex::new(ctx.take_range(first as usize, count as usize)));
    ctx.set(
        dst,
        Value::Enum {
            enum_name: variant.enum_name.clone(),
            variant: variant.variant.clone(),
            data,
        },
    )
}

fn load_enum(ctx: &mut StepCtx, dst: u16, info: u16) -> Flow {
    let variant = &ctx.cur.enum_variants[info as usize];
    ctx.set(
        dst,
        Value::Enum {
            enum_name: variant.enum_name.clone(),
            variant: variant.variant.clone(),
            data: Arc::new(Mutex::new(Vec::new())),
        },
    )
}

fn closure_op(ctx: &mut StepCtx, dst: u16, child: u16) -> Flow {
    let clo = make_closure(ctx, child);
    ctx.set(dst, Value::Closure(clo))
}

fn spawn_op(ctx: &mut StepCtx, dst: u16, child: u16) -> Flow {
    let clo = make_closure(ctx, child);
    let interp = ctx.vm.clone();
    let handle = ctx.vm.rt.spawn_blocking(move || {
        match interp.run_chunk(&clo.chunk, &[], &clo.captured) {
            Ok(v) => v,
            // A panic inside a task prints when it happens and makes the
            // join handle answer `Err(JoinError)`, the way real tokio does.
            // `resume_unwind` skips the default panic hook, so the printed
            // header is not doubled.
            Err(e) => {
                if let Some(p) = e.downcast_ref::<super::vm_support::ScriptPanic>() {
                    if p.file.is_empty() {
                        eprintln!("thread 'tokio-runtime-worker' panicked:");
                    } else {
                        eprintln!(
                            "thread 'tokio-runtime-worker' panicked at {}:{}:",
                            p.file, p.line
                        );
                    }
                    eprintln!("{}", p.rendered);
                    eprintln!(
                        "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace"
                    );
                } else {
                    eprintln!("rust error in task: {e:#}");
                }
                // The payload is the bare panic message, so the JoinError
                // the join handle answers formats exactly like real tokio's:
                // `task 11 panicked with message "boom"`.
                let payload = match e.downcast_ref::<super::vm_support::ScriptPanic>() {
                    Some(p) => {
                        let first = p.rendered.lines().next().unwrap_or_default();
                        first.strip_prefix("panicked: ").unwrap_or(first).to_string()
                    }
                    None => format!("{e:#}"),
                };
                std::panic::resume_unwind(Box::new(payload))
            }
        }
    });
    ctx.set(dst, Native::Task(handle).wrap())
}

fn make_closure(ctx: &mut StepCtx, child: u16) -> Arc<ClosureData> {
    let cur = ctx.cur;
    let child_chunk = cur.children[child as usize].clone();
    let caps = &cur.child_caps[child as usize];
    let captured: Vec<Upvalue> = caps
        .iter()
        .map(|c| match c {
            CapSource::Local(reg) => Upvalue::Value(ctx.stack[ctx.base + *reg as usize].clone()),
            CapSource::Upvalue(idx) | CapSource::MutableUpvalue(idx) => {
                ctx.upvalues()[*idx as usize].clone()
            }
            CapSource::MutableLocal(reg) => Upvalue::Mutable(ctx.cell(*reg).clone()),
        })
        .collect();
    Arc::new(ClosureData {
        chunk: child_chunk,
        captured,
    })
}

/// The storage a field or index access should hit. A reference base
/// resolves to its referent and a shared pointer auto-derefs to its
/// content, so `body.attributes` works when `body` is a borrow binding.
fn place_base(v: &Value) -> Result<Value> {
    Ok(match v {
        Value::Ref(reference) => reference
            .get()
            .ok_or_else(|| anyhow!("access through a dangling reference"))?,
        Value::Cell(_, slot) => slot.lock().clone(),
        other => other.clone(),
    })
}

fn set_index(ctx: &mut StepCtx, base: u16, key: u16, val: u16) -> Result<Flow> {
    // A range write into a string is the writeback of a mutating method
    // called on a string slice, like `s[2..].make_ascii_uppercase()`. A
    // string has no interior mutability, so the spliced value is stored
    // through the base itself: its register, its cell, or its reference.
    if let &Value::Range {
        start,
        end,
        inclusive,
    } = ctx.get(key)
    {
        let target = place_base(ctx.get(base))?;
        if let Value::Str(s) = &target {
            let new = Value::str(ops::splice_str(s, start, end, inclusive, ctx.get(val))?);
            let flow = match ctx.get(base).clone() {
                Value::Cell(_, slot) => {
                    *slot.lock() = new;
                    Flow::Next
                }
                Value::Ref(reference) => {
                    if !reference.set(new) {
                        bail!("assignment through a dangling reference");
                    }
                    Flow::Next
                }
                _ => ctx.set(base, new),
            };
            return Ok(flow);
        }
    }
    let target = place_base(ctx.get(base))?;
    ops::set_index(&target, ctx.get(key), ctx.get(val).clone())?;
    Ok(Flow::Next)
}

fn deref_op(ctx: &mut StepCtx, dst: u16, src: u16) -> Result<Flow> {
    let v = deref(ctx.get(src))?;
    Ok(ctx.set(dst, v))
}

fn deref(v: &Value) -> Result<Value> {
    Ok(match v {
        Value::Ref(reference) => reference
            .get()
            .ok_or_else(|| anyhow!("dereference of a dangling reference"))?,
        // `*rc` reads the content, the way real Deref does.
        Value::Cell(_, slot) => slot.lock().clone(),
        value => value.clone(),
    })
}

fn set_deref(ctx: &StepCtx, target: u16, val: u16) -> Result<Flow> {
    let Value::Ref(reference) = ctx.get(target) else {
        bail!("assignment through a non-reference value");
    };
    if !reference.set(ctx.get(val).clone()) {
        bail!("assignment through a dangling reference");
    }
    Ok(Flow::Next)
}

/// A value a fused compound assignment may read and write under the
/// referent's held lock: `apply_bin` on these is pure, it takes no lock and
/// runs no user code, and `user_op_type` never answers for them, so the
/// generic `Bin` op computes the identical result.
fn fusable_scalar(v: &Value) -> bool {
    matches!(
        v,
        Value::Int(_) | Value::IntW(..) | Value::Float(_) | Value::F32(_) | Value::Bool(_)
    )
}

/// `DerefBinAssign`: `*r op= v` as one op. When the slot and the operand
/// are both plain scalars the read-modify-write runs under the referent's
/// lock, so concurrent compound assignments through a shared cell, a tokio
/// mutex guard for one, cannot lose updates. Everything else runs the exact
/// sequence the unfused `Deref`, `Bin`, `SetDeref` ops ran, errors and
/// their order included.
fn deref_bin_assign(
    ctx: &mut StepCtx,
    target: u16,
    val: u16,
    op: super::bytecode::BinKind,
) -> Result<Flow> {
    if let Value::Ref(reference) = ctx.get(target)
        && fusable_scalar(ctx.get(val))
    {
        let reference = reference.clone();
        let b = ctx.get(val).clone();
        let fused = reference.update(|current| {
            if !fusable_scalar(current) {
                return Ok(false);
            }
            *current = apply_bin(op, current, &b)?;
            Ok(true)
        });
        match fused {
            Some(Ok(true)) => return Ok(Flow::Next),
            Some(Err(e)) => return Err(e),
            Some(Ok(false)) | None => {}
        }
    }
    let current = deref(ctx.get(target))?;
    let b = ctx.get(val).clone();
    let result = match user_bin(ctx, op, &current, &b)? {
        Some(v) => v,
        None => apply_bin(op, &current, &b)?,
    };
    let Value::Ref(reference) = ctx.get(target) else {
        bail!("assignment through a non-reference value");
    };
    if !reference.set(result) {
        bail!("assignment through a dangling reference");
    }
    Ok(Flow::Next)
}

/// `SetDerefParam`: a deref assignment whose target the compiler proved to be
/// a `&mut` parameter. A real reference is set through, and a plain value is
/// written into the parameter register, where the caller's writeback finds it.
fn set_deref_param(ctx: &mut StepCtx, target: u16, val: u16) -> Result<Flow> {
    if let Value::Ref(reference) = ctx.get(target) {
        if !reference.set(ctx.get(val).clone()) {
            bail!("assignment through a dangling reference");
        }
        return Ok(Flow::Next);
    }
    let value = ctx.get(val).clone();
    Ok(ctx.set(target, value))
}

fn get_field_op(ctx: &mut StepCtx, dst: u16, base: u16, member: u16) -> Result<Flow> {
    let target = place_base(ctx.get(base))?;
    let v = Vm::get_field(&target, &ctx.cur.members[member as usize])?;
    Ok(ctx.set(dst, v))
}

fn set_field_op(ctx: &StepCtx, base: u16, member: u16, val: u16) -> Result<Flow> {
    let target = place_base(ctx.get(base))?;
    Vm::set_field(
        &target,
        &ctx.cur.members[member as usize],
        ctx.get(val).clone(),
    )?;
    Ok(Flow::Next)
}

/// Make the field's value unique inside `base` and load it into `dst`
/// sharing the field's fresh storage. `base` was made unique by the ops the
/// compiler emits before this one, so the split cannot leak into a sibling
/// copy of the whole struct.
fn unique_reg(ctx: &mut StepCtx, reg: u16) -> Flow {
    ctx.stack[ctx.base + reg as usize].make_unique();
    Flow::Next
}

fn unique_field(ctx: &mut StepCtx, dst: u16, base: u16, member: u16) -> Result<Flow> {
    let member = &ctx.cur.members[member as usize];
    let target = place_base(ctx.get(base))?;
    let v = match (&target, member) {
        (Value::Struct(s), Member::Named(n)) => {
            let Some(i) = s.shape.slot(n) else {
                bail!("no field `{n}`");
            };
            let mut values = s.values.lock();
            values[i].make_unique();
            values[i].clone()
        }
        (Value::Struct(s), Member::Indexed(i)) => {
            let mut values = s.values.lock();
            let Some(slot) = values.get_mut(*i) else {
                bail!("no field {i}");
            };
            slot.make_unique();
            slot.clone()
        }
        (Value::Tuple(t), Member::Indexed(i)) => {
            let mut items = t.lock();
            let Some(slot) = items.get_mut(*i) else {
                bail!("no tuple index {i}");
            };
            slot.make_unique();
            slot.clone()
        }
        (recv, _) => Vm::get_field(recv, member)?,
    };
    Ok(ctx.set(dst, v))
}

/// The indexed-element version of `unique_field`. Anything that is not a
/// vec or map element read falls back to the plain index path, whose error
/// wording stays authoritative.
fn unique_index(ctx: &mut StepCtx, dst: u16, base: u16, key: u16) -> Result<Flow> {
    let target = place_base(ctx.get(base))?;
    let split = match (&target, ctx.get(key)) {
        (Value::Vec(list), key_val) => {
            match int_of(key_val).ok().and_then(|i| usize::try_from(i).ok()) {
                Some(i) => {
                    let mut items = list.lock();
                    items.get_mut(i).map(|slot| {
                        slot.make_unique();
                        slot.clone()
                    })
                }
                None => None,
            }
        }
        (Value::Map(map, _), key_val) => match key_val.as_key() {
            Some(k) => {
                let mut entries = map.lock();
                entries.get_mut(&k).map(|slot| {
                    slot.make_unique();
                    slot.clone()
                })
            }
            None => None,
        },
        _ => None,
    };
    // Anything that was not a plain element hit falls back to the ordinary
    // index path, whose error wording stays authoritative.
    let v = match split {
        Some(v) => v,
        None => ops::index(&target, ctx.get(key))?,
    };
    Ok(ctx.set(dst, v))
}

fn unique_cell(ctx: &mut StepCtx, dst: u16, cell: u16) -> Flow {
    let cell = ctx.cell(cell).clone();
    let v = {
        let mut slot = cell.lock();
        slot.make_unique();
        slot.clone()
    };
    ctx.set(dst, v)
}

fn unique_upvalue(ctx: &mut StepCtx, dst: u16, idx: u16) -> Flow {
    let v = match &ctx.upvalues()[idx as usize] {
        Upvalue::Value(v) => v.clone(),
        Upvalue::Mutable(cell) => {
            let mut slot = cell.lock();
            slot.make_unique();
            slot.clone()
        }
    };
    ctx.set(dst, v)
}

/// `&mut base[key]` as a real reference value. The compiler makes the
/// element unique first, so writes through the borrow stay private to the
/// borrowed place.
fn ref_index(ctx: &mut StepCtx, dst: u16, base: u16, key: u16) -> Result<Flow> {
    let target = place_base(ctx.get(base))?;
    let v = match (&target, ctx.get(key)) {
        (Value::Vec(list), key_val) => {
            let i = usize::try_from(int_of(key_val)?)?;
            let len = list.lock().len();
            if i >= len {
                bail!("index out of bounds: the len is {len} but the index is {i}");
            }
            Value::Ref(Arc::new(super::value::ValueRef::vec_element(
                list.clone(),
                i,
            )))
        }
        (Value::Map(map, _), key_val) => {
            let k = key_val.as_key().ok_or_else(|| anyhow!("invalid map key"))?;
            Value::Ref(Arc::new(super::value::ValueRef::map_entry(map.clone(), k)))
        }
        (recv, _) => bail!("cannot take `&mut` of an element of {}", recv.type_name()),
    };
    Ok(ctx.set(dst, v))
}

/// `&mut base.field` as a real reference value. A tuple field borrows as a
/// list element, tuples share the vec storage shape.
fn ref_field(ctx: &mut StepCtx, dst: u16, base: u16, member: u16) -> Result<Flow> {
    let member = &ctx.cur.members[member as usize];
    let target = place_base(ctx.get(base))?;
    let v = match (&target, member) {
        (Value::Struct(s), Member::Named(n)) => {
            let Some(slot) = s.shape.slot(n) else {
                bail!("no field `{n}`");
            };
            Value::Ref(Arc::new(super::value::ValueRef::struct_field(
                s.clone(),
                slot,
            )))
        }
        (Value::Struct(s), Member::Indexed(i)) => Value::Ref(Arc::new(
            super::value::ValueRef::struct_field(s.clone(), *i),
        )),
        (Value::Tuple(t), Member::Indexed(i)) => {
            Value::Ref(Arc::new(super::value::ValueRef::vec_element(t.clone(), *i)))
        }
        (recv, _) => bail!("cannot take `&mut` of a field of {}", recv.type_name()),
    };
    Ok(ctx.set(dst, v))
}

fn try_op(ctx: &mut StepCtx, dst: u16, src: u16) -> Flow {
    match ops::eval_try(ctx.get(src).clone()) {
        Ok(v) => ctx.set(dst, v),
        Err(early) => Flow::Ret(early),
    }
}

fn try_jump(ctx: &mut StepCtx, dst: u16, src: u16, to: u32) -> Flow {
    match ops::eval_try(ctx.get(src).clone()) {
        Ok(v) => {
            ctx.put(dst, v);
            Flow::Jump(to as usize)
        }
        // Falls through into the scope drops and the `Ret` emitted after
        // this op, with the early-return value ready in `dst`.
        Err(early) => ctx.set(dst, early),
    }
}

fn cast_op(ctx: &mut StepCtx, dst: u16, src: u16, ty: u16) -> Result<Flow> {
    let v = eval_cast(&ctx.cur.casts[ty as usize], ctx.get(src).clone())?;
    Ok(ctx.set(dst, v))
}

fn coerce_op(ctx: &mut StepCtx, dst: u16, src: u16, ty: u16) -> Flow {
    let v = ctx
        .vm
        .coerce_value(ctx.get(src).clone(), &ctx.cur.coerces[ty as usize]);
    ctx.set(dst, v)
}

fn test_bind(ctx: &mut StepCtx, val: u16, pat: u16, dst: u16) -> Flow {
    let info = &ctx.cur.pats[pat as usize];
    let raw = ctx.get(val).clone();
    // A reference scrutinee matches its referent, and its bindings borrow:
    // a composite binds wrapped as a borrow of the payload's own storage,
    // so mutation through the binding reaches the matched place, the way
    // `if let Some(v) = &mut opt { v.push(..) }` writes into `opt`.
    let (value, by_ref) = match &raw {
        Value::Ref(reference) => match reference.get() {
            Some(inner) => (inner, true),
            None => (Value::Unit, false),
        },
        _ => (raw, false),
    };
    let binds = &info.binds;
    let mut writes: Vec<(u16, Value)> = Vec::new();
    let matched = if by_ref {
        // Match first, then anchor each binding to the payload storage it
        // came from, so writes through the binding land in the place.
        let matched = try_bind(&info.pat, &value, &mut |_, _| {});
        if matched {
            let mut define = |name: &str, v: Value| {
                if let Some((_, reg)) = binds.iter().find(|(n, _)| n == name) {
                    writes.push((*reg, v));
                }
            };
            bind_pattern_refs(&info.pat, &value, &mut define);
        }
        matched
    } else {
        let mut define = |name: &str, v: Value| {
            if let Some((_, reg)) = binds.iter().find(|(n, _)| n == name) {
                writes.push((*reg, v));
            }
        };
        try_bind(&info.pat, &value, &mut define)
    };
    for (reg, v) in writes {
        ctx.put(reg, v);
    }
    ctx.set(dst, Value::Bool(matched))
}

fn fmt_op(ctx: &mut StepCtx, dst: u16, spec: u16) -> Result<Flow> {
    let text = ctx.vm.render_fmt(ctx.cur, spec, &ctx.stack[ctx.base..])?;
    Ok(ctx.set(dst, Value::str(text)))
}

fn macro_call(ctx: &mut StepCtx, kind: MacroKind, dst: u16, spec: u16) -> Result<Flow> {
    let text = ctx.vm.render_fmt(ctx.cur, spec, &ctx.stack[ctx.base..])?;
    Ok(match kind {
        MacroKind::Println => {
            println!("{text}");
            ctx.set(dst, Value::Unit)
        }
        MacroKind::Print => {
            print!("{text}");
            ctx.set(dst, Value::Unit)
        }
        MacroKind::Eprintln => {
            eprintln!("{text}");
            ctx.set(dst, Value::Unit)
        }
        MacroKind::Eprint => {
            eprint!("{text}");
            ctx.set(dst, Value::Unit)
        }
        MacroKind::Panic => bail!("panicked: {text}"),
        MacroKind::Anyhow => ctx.set(dst, Value::err(Value::str(text))),
        MacroKind::Bail => Flow::Ret(Value::err(Value::str(text))),
    })
}

fn dbg_op(ctx: &mut StepCtx, dst: u16, first: u16, argc: u16) -> Flow {
    let (first, argc) = (first as usize, argc as usize);
    let mut last = Value::Unit;
    for i in 0..argc {
        last = ctx.stack[ctx.base + first + i].clone();
        eprintln!("[dbg] {}", last.debug());
    }
    ctx.set(dst, last)
}

fn await_op(ctx: &mut StepCtx, dst: u16, src: u16) -> Result<Flow> {
    let v = ctx.take(src);
    Ok(ctx.set(dst, ctx.vm.await_value(v)?))
}

/// Apply an `as` cast to a value, with the same width semantics as
/// `eval_cast` in eval.rs.
fn eval_cast(target: &CastIr, v: Value) -> Result<Value> {
    let width = match target {
        CastIr::F64 => {
            return Ok(Value::Float(match v {
                Value::Int(i) => AsPrimitive::<f64>::as_(i),
                Value::IntW(..) => AsPrimitive::<f64>::as_(v.int_parts().unwrap().0),
                Value::Big(bits, w) => {
                    if w == super::numeric::IntWidth::U128 {
                        AsPrimitive::<f64>::as_(bits.cast_unsigned())
                    } else {
                        AsPrimitive::<f64>::as_(bits)
                    }
                }
                Value::Float(f) => f,
                Value::F32(f) => f64::from(f),
                other => bail!("cannot cast {} to float", other.type_name()),
            }));
        }
        CastIr::F32 => {
            return Ok(Value::F32(match v {
                Value::Int(i) => AsPrimitive::<f32>::as_(i),
                Value::IntW(..) => AsPrimitive::<f32>::as_(v.int_parts().unwrap().0),
                Value::Float(f) => AsPrimitive::<f32>::as_(f),
                Value::F32(f) => f,
                other => bail!("cannot cast {} to float", other.type_name()),
            }));
        }
        CastIr::Char => {
            return Ok(match v {
                Value::Int(i) => Value::Char(
                    u32::try_from(i)
                        .ok()
                        .and_then(char::from_u32)
                        .ok_or_else(|| anyhow!("invalid char code {i}"))?,
                ),
                Value::Char(c) => Value::Char(c),
                other => bail!("cannot cast {} to char", other.type_name()),
            });
        }
        CastIr::Unsupported(name) => bail!("unsupported cast target: {name}"),
        CastIr::Int(width) => *width,
    };
    let value = match v {
        Value::Int(i) => truncate(i128::from(i), width),
        Value::IntW(..) => truncate(v.int_parts().unwrap().0, width),
        // The stored i128 already carries the exact bit pattern, u128
        // included, so a narrowing cast keeps the low bits directly.
        Value::Big(bits, _) => truncate(bits, width),
        Value::Float(f) => float_to_int(f, width),
        Value::F32(f) => float_to_int(f64::from(f), width),
        Value::Char(c) => truncate(i128::from(c as u32), width),
        Value::Bool(b) => i128::from(b),
        other => bail!("cannot cast {} to integer", other.type_name()),
    };
    Ok(Value::int_of_width(value, width))
}
