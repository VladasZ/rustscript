//! One dispatch step of the register machine. `step` executes the op at
//! `ctx.ip` and answers with the `Flow` the frame loop in `vm.rs` applies:
//! fall through, jump, return, or push a call frame. The frame bookkeeping
//! stays in `exec`, the op bodies live here.

use std::collections::HashMap;
use std::iter::repeat_n;
use std::mem::take;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use num_traits::AsPrimitive;
use parking_lot::Mutex;

use super::bytecode::{CapSource, Chunk, MacroKind, Op, path_call_chunk};
use super::native::Native;
use super::numeric::{float_to_int, truncate};
use super::ops::{
    self, apply_bin, apply_bin_imm, apply_un, cmp_test, cmp_test_imm, int_of, try_bind,
};
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

    pub(super) fn cell(&self, reg: u16) -> Result<&Arc<Mutex<Value>>> {
        self.local_cells
            .get(&(self.base + reg as usize))
            .ok_or_else(|| anyhow!("missing mutable capture cell"))
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
        Op::LoadCell { dst, cell } => load_cell(ctx, *dst, *cell)?,
        Op::StoreCell { cell, src } => store_cell(ctx, *cell, *src)?,
        Op::StoreUpvalue { idx, src } => store_upvalue(ctx, *idx, *src)?,
        Op::Move { dst, src } => ctx.set(*dst, ctx.get(*src).clone()),
        Op::Bin { dst, a, b, op } => ctx.set(*dst, apply_bin(*op, ctx.get(*a), ctx.get(*b))?),
        Op::BinImm { dst, a, imm, op } => ctx.set(*dst, apply_bin_imm(*op, ctx.get(*a), *imm)?),
        Op::Un { dst, a, op } => ctx.set(*dst, apply_un(*op, ctx.get(*a))?),
        Op::Jump { to } => jump(ctx, *to as usize)?,
        Op::JumpIfFalse { cond, to } => branch(!ctx.get(*cond).is_truthy(), *to),
        Op::JumpIfTrue { cond, to } => branch(ctx.get(*cond).is_truthy(), *to),
        Op::CmpJump { a, b, op, to } => branch(!cmp_test(*op, ctx.get(*a), ctx.get(*b))?, *to),
        Op::CmpJumpImm { a, imm, op, to } => branch(!cmp_test_imm(*op, ctx.get(*a), *imm)?, *to),
        Op::CallFn {
            dst,
            func,
            base,
            argc,
            targ,
        } => call_fn(ctx, *dst, *func, *base, *argc, *targ)?,
        Op::CallValue {
            dst,
            callee,
            base,
            argc,
        } => call_value(ctx, *dst, *callee, *base, *argc)?,
        Op::CallPath {
            dst,
            path,
            base,
            argc,
        } => call_path(ctx, *dst, *path, *base, *argc)?,
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
        Op::IterInit { dst, src } => ctx.set(*dst, Vm::iterator_value(ctx.get(*src).clone())?),
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
        Op::Index { dst, base, key } => ctx.set(*dst, ops::index(ctx.get(*base), ctx.get(*key))?),
        Op::SetIndex { base, key, val } => set_index(ctx, *base, *key, *val)?,
        Op::Deref { dst, src } => ctx.set(*dst, deref(ctx.get(*src))?),
        Op::SetDeref { target, val } => set_deref(ctx, *target, *val)?,
        Op::GetField { dst, base, member } => get_field_op(ctx, *dst, *base, *member)?,
        Op::SetField { base, member, val } => set_field_op(ctx, *base, *member, *val)?,
        Op::Try { dst, src } => try_op(ctx, *dst, *src),
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

fn branch(jump: bool, to: u32) -> Flow {
    if jump {
        Flow::Jump(to as usize)
    } else {
        Flow::Next
    }
}

/// A backward jump closes a loop iteration, the moment to run a pending
/// Ctrl-C handler.
fn jump(ctx: &StepCtx, to: usize) -> Result<Flow> {
    if to <= ctx.ip {
        ctx.vm.run_pending_ctrlc()?;
    }
    Ok(Flow::Jump(to))
}

fn load_cell(ctx: &mut StepCtx, dst: u16, cell: u16) -> Result<Flow> {
    let v = ctx.cell(cell)?.lock().clone();
    Ok(ctx.set(dst, v))
}

fn store_cell(ctx: &StepCtx, cell: u16, src: u16) -> Result<Flow> {
    *ctx.cell(cell)?.lock() = ctx.get(src).clone();
    Ok(Flow::Next)
}

fn store_upvalue(ctx: &StepCtx, idx: u16, src: u16) -> Result<Flow> {
    if !ctx.upvalues()[idx as usize].set(ctx.get(src).clone()) {
        bail!("cannot assign to immutable capture");
    }
    Ok(Flow::Next)
}

fn call_fn(ctx: &StepCtx, dst: u16, func: u32, abase: u16, argc: u16, targ: u32) -> Result<Flow> {
    let callee = ctx.vm.functions[func as usize].clone();
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
    let i = match ctx.get(idx) {
        Value::Int(i) => *i,
        _ => unreachable!("for index is an integer"),
    };
    let item = match ctx.get(iter).clone() {
        Value::Native(iterator) => ctx.vm.iterator_next(&iterator)?,
        other => bail!("{} is not an iterator", other.type_name()),
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
    let data = ctx.take_range(first as usize, count as usize).into();
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
            data: Vec::new().into(),
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
        interp
            .run_chunk(&clo.chunk, &[], &clo.captured)
            .unwrap_or_else(|e| Value::err(Value::str(e.to_string())))
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
            CapSource::MutableLocal(reg) => {
                let slot = ctx.base + *reg as usize;
                let value = ctx.stack[slot].clone();
                let cell = ctx
                    .local_cells
                    .entry(slot)
                    .or_insert_with(|| Arc::new(Mutex::new(value)))
                    .clone();
                Upvalue::Mutable(cell)
            }
        })
        .collect();
    Arc::new(ClosureData {
        chunk: child_chunk,
        captured,
    })
}

fn set_index(ctx: &StepCtx, base: u16, key: u16, val: u16) -> Result<Flow> {
    ops::set_index(ctx.get(base), ctx.get(key), ctx.get(val).clone())?;
    Ok(Flow::Next)
}

fn deref(v: &Value) -> Result<Value> {
    Ok(match v {
        Value::Ref(reference) => reference
            .get()
            .ok_or_else(|| anyhow!("dereference of a dangling reference"))?,
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

fn get_field_op(ctx: &mut StepCtx, dst: u16, base: u16, member: u16) -> Result<Flow> {
    let v = Vm::get_field(ctx.get(base), &ctx.cur.members[member as usize])?;
    Ok(ctx.set(dst, v))
}

fn set_field_op(ctx: &StepCtx, base: u16, member: u16, val: u16) -> Result<Flow> {
    Vm::set_field(
        ctx.get(base),
        &ctx.cur.members[member as usize],
        ctx.get(val).clone(),
    )?;
    Ok(Flow::Next)
}

fn try_op(ctx: &mut StepCtx, dst: u16, src: u16) -> Flow {
    match ops::eval_try(ctx.get(src).clone()) {
        Ok(v) => ctx.set(dst, v),
        Err(early) => Flow::Ret(early),
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
    let value = ctx.get(val).clone();
    let binds = &info.binds;
    let mut writes: Vec<(u16, Value)> = Vec::new();
    let matched = {
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
    let text = Vm::render_fmt(ctx.cur, spec, &ctx.stack[ctx.base..])?;
    Ok(ctx.set(dst, Value::str(text)))
}

fn macro_call(ctx: &mut StepCtx, kind: MacroKind, dst: u16, spec: u16) -> Result<Flow> {
    let text = Vm::render_fmt(ctx.cur, spec, &ctx.stack[ctx.base..])?;
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
        Value::Float(f) => float_to_int(f, width),
        Value::F32(f) => float_to_int(f64::from(f), width),
        Value::Char(c) => truncate(i128::from(c as u32), width),
        Value::Bool(b) => i128::from(b),
        other => bail!("cannot cast {} to integer", other.type_name()),
    };
    Ok(Value::int_of_width(value, width))
}
