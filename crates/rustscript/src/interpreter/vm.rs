//! The register machine. Executes a compiled `Chunk` against one contiguous
//! register stack. Calls to user functions and closures push a frame record
//! and continue in the same instruction loop, so a script-level call costs no
//! native recursion, no allocation, and no register file copy beyond its
//! arguments. Anything else, methods and std or crate bridges, is delegated to
//! the existing dispatch on `Interp` with already evaluated values.

use std::cell::RefCell;
use std::mem::{replace, take};
use std::rc::Rc;

use anyhow::{Result, bail};

use super::bytecode::{BuiltinId, CapSource, Chunk, DISCARD, MacroKind, MethodName, Op};
use super::value::{ClosureData, StructShape, Value};
use super::Interp;

/// Guard against runaway recursion, since script calls no longer consume the
/// native stack. Depth, not registers, so deep-but-narrow recursion still works.
const MAX_CALL_DEPTH: usize = 100_000;

/// A binding of a generic parameter name to the concrete type a caller passed
/// by turbofish, plus the module that type was named in, so the callee can
/// resolve `from_str::<T>` to the real struct. Empty for a non-generic call.
pub(super) type TypeEnv = Rc<[(Rc<str>, Rc<syn::Type>, u16)]>;

fn empty_type_env() -> TypeEnv {
    Rc::from(Vec::new())
}

/// A suspended caller, restored when the callee returns.
struct Frame {
    chunk: Rc<Chunk>,
    closure: Option<Rc<ClosureData>>,
    ip: usize,
    base: usize,
    dst: u16,
    type_env: TypeEnv,
}

thread_local! {
    /// Recycled register stacks, so re-entering the VM from a bridge, for
    /// example a closure passed to `map`, does not allocate a fresh `Vec`.
    static STACK_POOL: RefCell<Vec<Vec<Value>>> = const { RefCell::new(Vec::new()) };
}

fn take_stack() -> Vec<Value> {
    STACK_POOL.with(|p| p.borrow_mut().pop()).unwrap_or_default()
}

fn recycle_stack(mut s: Vec<Value>) {
    s.clear();
    STACK_POOL.with(|p| {
        let mut p = p.borrow_mut();
        if p.len() < 32 {
            p.push(s);
        }
    });
}

impl Interp {
    /// Run a compiled function or closure body with a checked argument count.
    pub(super) fn run_chunk(
        &self,
        chunk: &Rc<Chunk>,
        args: &[Value],
        upvalues: &[Value],
    ) -> Result<Value> {
        if args.len() != chunk.num_params {
            bail!("`{}` expects {} args but got {}", chunk.name, chunk.num_params, args.len());
        }
        let mut stack = take_stack();
        stack.resize(chunk.num_regs.max(chunk.num_params), Value::Unit);
        for (i, a) in args.iter().enumerate() {
            stack[i] = a.clone();
        }
        let result = self.exec(chunk, &mut stack, upvalues);
        recycle_stack(stack);
        result
    }

    fn exec(
        &self,
        entry: &Rc<Chunk>,
        stack: &mut Vec<Value>,
        entry_upvalues: &[Value],
    ) -> Result<Value> {
        let mut frames: Vec<Frame> = Vec::new();
        let mut cur = entry.clone();
        let mut cur_clo: Option<Rc<ClosureData>> = None;
        let mut cur_tenv: TypeEnv = empty_type_env();
        let mut base = 0usize;
        let mut ip = 0usize;

        // Return `$v` from the current script function: pop back into the
        // caller frame, or leave the VM when this was the entry chunk.
        macro_rules! ret {
            ($v:expr) => {{
                let v = $v;
                match frames.pop() {
                    None => return Ok(v),
                    Some(f) => {
                        cur = f.chunk;
                        cur_clo = f.closure;
                        cur_tenv = f.type_env;
                        ip = f.ip;
                        base = f.base;
                        set_reg(&mut stack[base + f.dst as usize], v);
                        continue;
                    }
                }
            }};
        }

        // Enter `$chunk` with `$argc` args taken from the caller window at
        // `$abase`, storing the result into caller register `$dst` on return.
        macro_rules! call {
            ($chunk:expr, $clo:expr, $dst:expr, $abase:expr, $argc:expr) => {
                call!($chunk, $clo, $dst, $abase, $argc, empty_type_env())
            };
            ($chunk:expr, $clo:expr, $dst:expr, $abase:expr, $argc:expr, $tenv:expr) => {{
                let callee: Rc<Chunk> = $chunk;
                if $argc != callee.num_params {
                    bail!(
                        "`{}` expects {} args but got {}",
                        callee.name,
                        callee.num_params,
                        $argc
                    );
                }
                if frames.len() >= MAX_CALL_DEPTH {
                    bail!("stack overflow: call depth exceeded {MAX_CALL_DEPTH}");
                }
                let nbase = base + cur.num_regs;
                let need = nbase + callee.num_regs.max(callee.num_params);
                if stack.len() < need {
                    stack.resize(need, Value::Unit);
                }
                for i in 0..$argc {
                    let v = take(&mut stack[base + $abase + i]);
                    set_reg(&mut stack[nbase + i], v);
                }
                // Frames are not truncated on return, so clear whatever the
                // previous occupant left in the non-argument slots.
                for slot in &mut stack[nbase + $argc..need] {
                    set_reg(slot, Value::Unit);
                }
                frames.push(Frame {
                    chunk: replace(&mut cur, callee),
                    closure: replace(&mut cur_clo, $clo),
                    type_env: replace(&mut cur_tenv, $tenv),
                    ip: ip + 1,
                    base,
                    dst: $dst,
                });
                base = nbase;
                ip = 0;
                continue;
            }};
        }

        loop {
            if ip >= cur.code.len() {
                ret!(Value::Unit);
            }
            match &cur.code[ip] {
                Op::LoadConst { dst, k } => {
                    let v = Value::from_const(&cur.consts[*k as usize]);
                    set_reg(&mut stack[base + *dst as usize], v);
                }
                Op::LoadInt { dst, v } => stack[base + *dst as usize] = Value::Int(*v),
                Op::LoadBool { dst, v } => stack[base + *dst as usize] = Value::Bool(*v),
                Op::LoadUnit { dst } => stack[base + *dst as usize] = Value::Unit,
                Op::LoadGlobal { dst, idx } => {
                    let v = self.global(*idx as usize)?;
                    set_reg(&mut stack[base + *dst as usize], v);
                }
                Op::LoadUpvalue { dst, idx } => {
                    let upvals: &[Value] = match &cur_clo {
                        Some(c) => &c.captured,
                        None => entry_upvalues,
                    };
                    set_reg(&mut stack[base + *dst as usize], upvals[*idx as usize].clone());
                }
                Op::Move { dst, src } => {
                    let v = stack[base + *src as usize].clone();
                    set_reg(&mut stack[base + *dst as usize], v);
                }

                Op::Bin { dst, a, b, op } => {
                    let v = apply_bin(*op, &stack[base + *a as usize], &stack[base + *b as usize])?;
                    set_reg(&mut stack[base + *dst as usize], v);
                }
                Op::BinImm { dst, a, imm, op } => {
                    let v = apply_bin_imm(*op, &stack[base + *a as usize], *imm)?;
                    set_reg(&mut stack[base + *dst as usize], v);
                }
                Op::Un { dst, a, op } => {
                    let v = apply_un(*op, &stack[base + *a as usize])?;
                    set_reg(&mut stack[base + *dst as usize], v);
                }

                Op::Jump { to } => {
                    let to = *to as usize;
                    if to <= ip {
                        self.run_pending_ctrlc()?;
                    }
                    ip = to;
                    continue;
                }
                Op::JumpIfFalse { cond, to } => {
                    if !stack[base + *cond as usize].is_truthy() {
                        ip = *to as usize;
                        continue;
                    }
                }
                Op::JumpIfTrue { cond, to } => {
                    if stack[base + *cond as usize].is_truthy() {
                        ip = *to as usize;
                        continue;
                    }
                }
                Op::CmpJump { a, b, op, to } => {
                    if !cmp_test(*op, &stack[base + *a as usize], &stack[base + *b as usize])? {
                        ip = *to as usize;
                        continue;
                    }
                }
                Op::CmpJumpImm { a, imm, op, to } => {
                    if !cmp_test_imm(*op, &stack[base + *a as usize], *imm)? {
                        ip = *to as usize;
                        continue;
                    }
                }

                Op::CallFn { dst, func, base: abase, argc, targ } => {
                    let (dst, func) = (*dst, *func as usize);
                    let (abase, argc) = (*abase as usize, *argc as usize);
                    let callee = self.functions[func].clone();
                    // Bind the call's turbofish type args to the callee's
                    // generic parameters, resolved in this (caller) module.
                    let tenv: TypeEnv = if *targ != u32::MAX {
                        let targs = &cur.call_type_args[*targ as usize];
                        let module = cur.module;
                        callee
                            .generics
                            .iter()
                            .zip(targs.iter())
                            .map(|(name, ty)| (name.clone(), ty.clone(), module))
                            .collect()
                    } else {
                        empty_type_env()
                    };
                    call!(callee, None, dst, abase, argc, tenv);
                }
                Op::CallValue { dst, callee, base: abase, argc } => {
                    let (dst, callee) = (*dst, *callee as usize);
                    let (abase, argc) = (*abase as usize, *argc as usize);
                    let clo = match &stack[base + callee] {
                        Value::Closure(clo) => clo.clone(),
                        other => bail!("cannot call {}", other.type_name()),
                    };
                    let chunk = clo.chunk.clone();
                    call!(chunk, Some(clo), dst, abase, argc);
                }
                Op::CallPath { dst, path, base: abase, argc } => {
                    let (dst, path) = (*dst, *path as usize);
                    let (abase, argc) = (*abase as usize, *argc as usize);
                    let (segs, coerce) = &cur.paths[path];
                    if let Some(v) = self.internal_path(segs, &stack[base..], abase, argc)? {
                        set_reg(&mut stack[base + dst as usize], v);
                    } else {
                        let args = take_range(stack, base + abase, argc);
                        // Typed json parses straight into the target structs,
                        // no generic tree and no coercion pass afterwards.
                        if let Some(ty) = coerce {
                            let canon = self.canonical(segs);
                            if canon.len() >= 2
                                && canon[canon.len() - 2] == "serde_json"
                                && canon[canon.len() - 1] == "from_str"
                            {
                                let v = self.typed_from_str(&args, ty, cur.module as usize, &cur_tenv)?;
                                set_reg(&mut stack[base + dst as usize], v);
                                ip += 1;
                                continue;
                            }
                        }
                        let mut v = self.dispatch_call(segs, args)?;
                        if let Some(ty) = coerce {
                            v = self.coerce_result(v, ty, cur.module as usize);
                        }
                        set_reg(&mut stack[base + dst as usize], v);
                    }
                }
                Op::PathValue { dst, path } => {
                    let (segs, _) = &cur.paths[*path as usize];
                    set_reg(&mut stack[base + *dst as usize], self.eval_path_value(segs)?);
                }
                Op::Method { dst, recv, name, base: abase, argc } => {
                    let (dst, recv) = (*dst, *recv as usize);
                    let (abase, argc) = (*abase as usize, *argc as usize);
                    let name = &cur.names[*name as usize];
                    let s = base + abase;
                    // Strings are copy on write, so push must edit the Rc in
                    // the receiver register itself. Going through the normal
                    // path would edit a copy and drop the change.
                    if matches!(name.id, BuiltinId::Push | BuiltinId::PushStr)
                        && matches!(stack[base + recv], Value::Str(_))
                    {
                        let Value::Str(mut buf) = take(&mut stack[base + recv]) else {
                            unreachable!()
                        };
                        {
                            let out = Value::str_make_mut(&mut buf);
                            match (&name.id, stack[s..s + argc].first()) {
                                (BuiltinId::Push, Some(Value::Char(c))) => out.push(*c),
                                (BuiltinId::PushStr, Some(arg)) => out.push_str(&arg.display()),
                                _ => {}
                            }
                        }
                        set_reg(&mut stack[base + recv], Value::Str(buf));
                        if dst != DISCARD {
                            set_reg(&mut stack[base + dst as usize], Value::Unit);
                        }
                        ip += 1;
                        continue;
                    }
                    // Option and Result accessors dominate counting loops, so
                    // their success paths run right here, skipping the whole
                    // dispatch chain. Failure paths fall through and get their
                    // errors from the slow path. Skipped when the script
                    // defines methods, which could shadow these on an enum.
                    if self.methods.is_empty()
                        && matches!(
                            name.id,
                            BuiltinId::Copied | BuiltinId::Unwrap | BuiltinId::UnwrapOr
                        )
                    {
                        // 0 none, 1 clone receiver, 2 clone payload, 3 default
                        let choice = match &stack[base + recv] {
                            Value::Enum { enum_name, variant, .. } => {
                                if matches!(name.id, BuiltinId::Copied) {
                                    if &**enum_name == "Option" { 1 } else { 0 }
                                } else if !matches!(&**enum_name, "Option" | "Result") {
                                    0
                                } else if matches!(&**variant, "Some" | "Ok") {
                                    2
                                } else if matches!(name.id, BuiltinId::UnwrapOr) {
                                    3
                                } else {
                                    0
                                }
                            }
                            _ => 0,
                        };
                        if choice != 0 {
                            let v = match choice {
                                1 => stack[base + recv].clone(),
                                2 => match &stack[base + recv] {
                                    Value::Enum { data, .. } => {
                                        data.first().cloned().unwrap_or(Value::Unit)
                                    }
                                    _ => unreachable!(),
                                },
                                _ => {
                                    if argc > 0 {
                                        take(&mut stack[s])
                                    } else {
                                        Value::Unit
                                    }
                                }
                            };
                            if dst != DISCARD {
                                set_reg(&mut stack[base + dst as usize], v);
                            }
                            ip += 1;
                            continue;
                        }
                    }
                    // to_string and clone on a string are a refcount bump,
                    // not worth the dispatch walk.
                    if matches!(name.id, BuiltinId::ToString | BuiltinId::Clone)
                        && let Value::Str(v) = &stack[base + recv]
                    {
                        if dst != DISCARD {
                            let v = Value::Str(v.clone());
                            set_reg(&mut stack[base + dst as usize], v);
                        }
                        ip += 1;
                        continue;
                    }
                    // Map get and insert run inline for the same reason as
                    // the Option accessors above. User methods cannot exist
                    // on a HashMap, so no gate is needed.
                    if matches!(
                        name.id,
                        BuiltinId::Get | BuiltinId::Insert | BuiltinId::ContainsKey
                    ) && matches!(stack[base + recv], Value::Map(_))
                        && argc >= 1
                        && base + recv < s
                    {
                        let (lo, hi) = stack.split_at_mut(s);
                        let Value::Map(m) = &lo[base + recv] else { unreachable!() };
                        let v = match name.id {
                            BuiltinId::Insert => {
                                let k = take(&mut hi[0]).into_key();
                                let Some(k) = k else {
                                    bail!("invalid map key")
                                };
                                let val = if argc > 1 { take(&mut hi[1]) } else { Value::Unit };
                                let old = m.borrow_mut().insert(k, val);
                                if dst == DISCARD {
                                    Value::Unit
                                } else {
                                    match old {
                                        Some(old) => Value::some(old),
                                        None => Value::none(),
                                    }
                                }
                            }
                            _ => {
                                let Some(k) = hi[0].key_ref() else {
                                    bail!("invalid map key")
                                };
                                if matches!(name.id, BuiltinId::ContainsKey) {
                                    Value::Bool(m.borrow().get(&k).is_some())
                                } else {
                                    match m.borrow().get(&k).cloned() {
                                        Some(v) => Value::some(v),
                                        None => Value::none(),
                                    }
                                }
                            }
                        };
                        if dst != DISCARD {
                            set_reg(&mut stack[base + dst as usize], v);
                        }
                        ip += 1;
                        continue;
                    }
                    // The arg window holds dead temporaries, so methods may
                    // consume them in place without cloning. The window sits
                    // above the receiver register, so the split hands out the
                    // receiver by reference and the args mutably at once.
                    let v = if argc == 0 {
                        self.eval_method(&stack[base + recv], name, &mut [])?
                    } else if base + recv < s {
                        let (lo, hi) = stack.split_at_mut(s);
                        self.eval_method(&lo[base + recv], name, &mut hi[..argc])?
                    } else {
                        let recv_v = stack[base + recv].clone();
                        self.eval_method(&recv_v, name, &mut stack[s..s + argc])?
                    };
                    if dst != DISCARD {
                        set_reg(&mut stack[base + dst as usize], v);
                    }
                }
                Op::GetOrDefault { dst, recv, key, default } => {
                    let (r, k) = (base + *recv as usize, base + *key as usize);
                    let df = base + *default as usize;
                    // Key and default may live in variable registers, so they
                    // are cloned, never taken.
                    let v = match &stack[r] {
                        Value::Map(m) => {
                            let Some(kr) = stack[k].key_ref() else {
                                bail!("invalid map key")
                            };
                            m.borrow().get(&kr).cloned()
                        }
                        Value::Vec(items) => match &stack[k] {
                            Value::Int(i) => {
                                usize::try_from(*i).ok().and_then(|i| items.borrow().get(i).cloned())
                            }
                            other => bail!("cannot index a vector with {}", other.type_name()),
                        },
                        _ => {
                            let recv_v = stack[r].clone();
                            let get = MethodName { text: "get".into(), id: BuiltinId::Get };
                            let opt = self.eval_method(&recv_v, &get, &mut [stack[k].clone()])?;
                            let copied =
                                MethodName { text: "copied".into(), id: BuiltinId::Copied };
                            let opt = self.eval_method(&opt, &copied, &mut [])?;
                            let uo =
                                MethodName { text: "unwrap_or".into(), id: BuiltinId::UnwrapOr };
                            Some(self.eval_method(&opt, &uo, &mut [stack[df].clone()])?)
                        }
                    };
                    let v = match v {
                        Some(v) => v,
                        None => stack[df].clone(),
                    };
                    set_reg(&mut stack[base + *dst as usize], v);
                }
                Op::Ret { src } => {
                    let v = take(&mut stack[base + *src as usize]);
                    ret!(v);
                }

                Op::MakeVec { dst, base: wbase, count } => {
                    let (dst, wbase, count) = (*dst, *wbase as usize, *count as usize);
                    let items = take_range(stack, base + wbase, count);
                    set_reg(&mut stack[base + dst as usize], Value::vec(items));
                }
                Op::MakeTuple { dst, base: wbase, count } => {
                    let (dst, wbase, count) = (*dst, *wbase as usize, *count as usize);
                    let items = take_range(stack, base + wbase, count);
                    set_reg(&mut stack[base + dst as usize], Value::Tuple(Rc::new(RefCell::new(items))));
                }
                Op::MakeArrayRepeat { dst, val, count } => {
                    let n = match &stack[base + *count as usize] {
                        Value::Int(n) => *n as usize,
                        _ => bail!("array repeat length must be an integer"),
                    };
                    let v = stack[base + *val as usize].clone();
                    set_reg(&mut stack[base + *dst as usize], Value::vec(std::iter::repeat_n(v, n).collect()));
                }
                Op::MakeRange { dst, start, end, inclusive } => {
                    let s = int_of(&stack[base + *start as usize], "range bound")?;
                    let e = int_of(&stack[base + *end as usize], "range bound")?;
                    set_reg(
                        &mut stack[base + *dst as usize],
                        Value::Range { start: s, end: e, inclusive: *inclusive },
                    );
                }
                Op::IterInit { dst, src } => {
                    let src_v = stack[base + *src as usize].clone();
                    let it = match src_v {
                        // Ranges are stepped in place by ForNext, never
                        // materialized into a Vec.
                        Value::Range { .. } => src_v,
                        other => Value::vec(self.into_iter_items(other)?),
                    };
                    set_reg(&mut stack[base + *dst as usize], it);
                }
                Op::ForNext { iter, idx, val, to } => {
                    let i = match &stack[base + *idx as usize] {
                        Value::Int(i) => *i,
                        _ => unreachable!("for index is an integer"),
                    };
                    let item = match &stack[base + *iter as usize] {
                        Value::Vec(items) => items.borrow().get(i as usize).cloned(),
                        Value::Range { start, end, inclusive } => {
                            let n = start + i;
                            let done = if *inclusive { n > *end } else { n >= *end };
                            if done { None } else { Some(Value::Int(n)) }
                        }
                        _ => None,
                    };
                    match item {
                        Some(v) => {
                            set_reg(&mut stack[base + *val as usize], v);
                            self.run_pending_ctrlc()?;
                            set_reg(&mut stack[base + *idx as usize], Value::Int(i + 1));
                        }
                        None => {
                            ip = *to as usize;
                            continue;
                        }
                    }
                }
                Op::MakeStruct { dst, info, base: wbase } => {
                    let (dst, wbase) = (*dst, *wbase as usize);
                    let lit = &cur.struct_lits[*info as usize];
                    let written = lit.shape.fields.len();
                    let mut values: Vec<Value> = (0..written)
                        .map(|k| take(&mut stack[base + wbase + k]))
                        .collect();
                    // The shape is prebuilt at compile time and shared by every
                    // instance from this literal. A `..rest` adds fields the
                    // literal did not write, so that case builds a merged shape.
                    let v = if lit.has_rest {
                        let rest = &stack[base + wbase + written];
                        let mut fields = lit.shape.fields.clone();
                        let mut renames = lit.shape.renames.clone();
                        if let Value::Struct(r) = rest {
                            let rvals = r.values.borrow();
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
                        Value::structure(
                            StructShape::with_renames(lit.shape.name.clone(), fields, renames),
                            values,
                        )
                    } else {
                        Value::structure(lit.shape.clone(), values)
                    };
                    set_reg(&mut stack[base + dst as usize], v);
                }
                Op::MakeClosure { dst, child } => {
                    let child_chunk = cur.children[*child as usize].clone();
                    let caps = &cur.child_caps[*child as usize];
                    let upvals: &[Value] = match &cur_clo {
                        Some(c) => &c.captured,
                        None => entry_upvalues,
                    };
                    let captured: Vec<Value> = caps
                        .iter()
                        .map(|c| match c {
                            CapSource::Local(reg) => stack[base + *reg as usize].clone(),
                            CapSource::Upvalue(idx) => upvals[*idx as usize].clone(),
                        })
                        .collect();
                    set_reg(
                        &mut stack[base + *dst as usize],
                        Value::Closure(Rc::new(ClosureData { chunk: child_chunk, captured })),
                    );
                }

                Op::Index { dst, base: b, key } => {
                    let v = self.index(&stack[base + *b as usize], &stack[base + *key as usize])?;
                    set_reg(&mut stack[base + *dst as usize], v);
                }
                Op::SetIndex { base: b, key, val } => {
                    self.set_index(
                        &stack[base + *b as usize],
                        &stack[base + *key as usize],
                        stack[base + *val as usize].clone(),
                    )?;
                }
                Op::GetField { dst, base: b, member } => {
                    let v =
                        self.get_field(&stack[base + *b as usize], &cur.members[*member as usize])?;
                    set_reg(&mut stack[base + *dst as usize], v);
                }
                Op::SetField { base: b, member, val } => {
                    self.set_field(
                        &stack[base + *b as usize],
                        &cur.members[*member as usize],
                        stack[base + *val as usize].clone(),
                    )?;
                }

                Op::Try { dst, src } => {
                    match self.eval_try(stack[base + *src as usize].clone())? {
                        Ok(v) => stack[base + *dst as usize] = v,
                        Err(early) => ret!(early),
                    }
                }
                Op::Cast { dst, src, ty } => {
                    let v = self
                        .eval_cast(stack[base + *src as usize].clone(), &cur.casts[*ty as usize])?;
                    set_reg(&mut stack[base + *dst as usize], v);
                }
                Op::Coerce { dst, src, ty } => {
                    let v = self.coerce_value(
                        stack[base + *src as usize].clone(),
                        &cur.casts[*ty as usize],
                        cur.module as usize,
                    );
                    set_reg(&mut stack[base + *dst as usize], v);
                }

                Op::TestBind { val, pat, dst } => {
                    let info = &cur.pats[*pat as usize];
                    let value = stack[base + *val as usize].clone();
                    let binds = &info.binds;
                    let matched = {
                        let mut define = |name: &str, v: Value| {
                            if let Some((_, reg)) = binds.iter().find(|(n, _)| n == name) {
                                set_reg(&mut stack[base + *reg as usize], v);
                            }
                        };
                        try_bind(&info.pat, &value, &mut define)
                    };
                    set_reg(&mut stack[base + *dst as usize], Value::Bool(matched));
                }

                Op::Fmt { dst, spec } => {
                    let text = self.render_fmt(&cur, *spec, &stack[base..])?;
                    set_reg(&mut stack[base + *dst as usize], Value::str(text));
                }
                Op::MacroCall { kind, dst, spec } => {
                    let text = self.render_fmt(&cur, *spec, &stack[base..])?;
                    match kind {
                        MacroKind::Println => println!("{text}"),
                        MacroKind::Print => print!("{text}"),
                        MacroKind::Eprintln => eprintln!("{text}"),
                        MacroKind::Eprint => eprint!("{text}"),
                        MacroKind::Panic => bail!("panicked: {text}"),
                        MacroKind::Anyhow => {
                            set_reg(&mut stack[base + *dst as usize], Value::err(Value::str(text)));
                        }
                        MacroKind::Bail => {
                            ret!(Value::err(Value::str(text)));
                        }
                    }
                    if !matches!(kind, MacroKind::Anyhow) {
                        set_reg(&mut stack[base + *dst as usize], Value::Unit);
                    }
                }
                Op::Spawn { .. } | Op::Await { .. } => {
                    bail!("async is only available under #[tokio::main]")
                }
                Op::Dbg { dst, base: wbase, argc } => {
                    let (dst, wbase, argc) = (*dst, *wbase as usize, *argc as usize);
                    let mut last = Value::Unit;
                    for i in 0..argc {
                        last = stack[base + wbase + i].clone();
                        eprintln!("[dbg] {}", last.debug());
                    }
                    set_reg(&mut stack[base + dst as usize], last);
                }
            }
            ip += 1;
        }
    }

    /// Call a closure value with already evaluated arguments. Used by the
    /// higher-order bridges and the Ctrl-C handler.
    pub(super) fn call_closure(&self, clo: &ClosureData, args: &[Value]) -> Result<Value> {
        self.run_chunk(&clo.chunk, args, &clo.captured)
    }

    /// Handle the compiler's internal marker paths. Returns None for a normal
    /// path so the caller falls through to the bridge dispatch.
    fn internal_path(
        &self,
        segs: &[String],
        regs: &[Value],
        base: usize,
        argc: usize,
    ) -> Result<Option<Value>> {
        let head = segs.first().map(|s| s.as_str()).unwrap_or("");
        match head {
            "::unreachable_match" => bail!("no match arm matched the value"),
            "::assert_failed" => bail!("assertion failed"),
            "::ensure_fail" => {
                let msg = if argc > 0 {
                    regs[base].display()
                } else {
                    "condition failed".to_string()
                };
                Ok(Some(Value::err(Value::str(msg))))
            }
            _ => Ok(None),
        }
    }

    fn render_fmt(&self, chunk: &Chunk, spec: u16, regs: &[Value]) -> Result<String> {
        let f = &chunk.fmts[spec as usize];
        let positional: Vec<Value> = f.positional.iter().map(|r| regs[*r as usize].clone()).collect();
        let named: Vec<(String, Value)> = f
            .named
            .iter()
            .map(|(n, r)| (n.clone(), regs[*r as usize].clone()))
            .collect();
        super::format::render_values(&f.template, &positional, &named)
    }
}

/// Move `count` registers out of the window at `s`, leaving `Unit` behind. The
/// compiler only builds windows out of dead temporaries, so taking is safe and
/// skips a clone plus the matching drop.
fn take_range(stack: &mut [Value], s: usize, count: usize) -> Vec<Value> {
    (0..count).map(|i| take(&mut stack[s + i])).collect()
}

/// Write a register. A plain old value has no drop glue, but the compiler
/// still emits an out of line `drop_in_place` call for the write, and that
/// call is one of the hottest lines in profiles. Check the old value inline
/// and forget it when dropping would do nothing anyway.
#[inline(always)]
pub(super) fn set_reg(slot: &mut Value, v: Value) {
    if matches!(
        slot,
        Value::Unit
            | Value::Bool(_)
            | Value::Int(_)
            | Value::Float(_)
            | Value::Char(_)
            | Value::Range { .. }
    ) {
        std::mem::forget(replace(slot, v));
    } else {
        *slot = v;
    }
}

fn int_of(v: &Value, what: &str) -> Result<i64> {
    match v {
        Value::Int(i) => Ok(*i),
        _ => bail!("{what} must be an integer"),
    }
}


use super::ops::{apply_bin, apply_bin_imm, apply_un, cmp_test, cmp_test_imm, try_bind};
