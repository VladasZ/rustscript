//! The register machine. Executes a compiled `Chunk` against one contiguous
//! register stack. Calls to user functions and closures push a frame record
//! and continue in the same instruction loop, so a script-level call costs no
//! native recursion, no allocation, and no register file copy beyond its
//! arguments. Anything else, methods and std or crate bridges, is delegated to
//! the existing dispatch on `Interp` with already evaluated values.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::mem::{replace, take};
use std::rc::Rc;

use anyhow::{Result, anyhow, bail};
use syn::{Lit, Pat};

use super::bytecode::{BinKind, CapSource, Chunk, MacroKind, Op, UnKind};
use super::value::{ClosureData, Value, fields_with_capacity};
use super::Interp;

/// Guard against runaway recursion, since script calls no longer consume the
/// native stack. Depth, not registers, so deep-but-narrow recursion still works.
const MAX_CALL_DEPTH: usize = 100_000;

/// A suspended caller, restored when the callee returns.
struct Frame {
    chunk: Rc<Chunk>,
    closure: Option<Rc<ClosureData>>,
    ip: usize,
    base: usize,
    dst: u16,
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
                        ip = f.ip;
                        base = f.base;
                        stack[base + f.dst as usize] = v;
                        continue;
                    }
                }
            }};
        }

        // Enter `$chunk` with `$argc` args taken from the caller window at
        // `$abase`, storing the result into caller register `$dst` on return.
        macro_rules! call {
            ($chunk:expr, $clo:expr, $dst:expr, $abase:expr, $argc:expr) => {{
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
                    stack[nbase + i] = take(&mut stack[base + $abase + i]);
                }
                // Frames are not truncated on return, so clear whatever the
                // previous occupant left in the non-argument slots.
                for slot in &mut stack[nbase + $argc..need] {
                    *slot = Value::Unit;
                }
                frames.push(Frame {
                    chunk: replace(&mut cur, callee),
                    closure: replace(&mut cur_clo, $clo),
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
                    stack[base + *dst as usize] = cur.consts[*k as usize].clone();
                }
                Op::LoadInt { dst, v } => stack[base + *dst as usize] = Value::Int(*v),
                Op::LoadBool { dst, v } => stack[base + *dst as usize] = Value::Bool(*v),
                Op::LoadUnit { dst } => stack[base + *dst as usize] = Value::Unit,
                Op::LoadUpvalue { dst, idx } => {
                    let upvals: &[Value] = match &cur_clo {
                        Some(c) => &c.captured,
                        None => entry_upvalues,
                    };
                    stack[base + *dst as usize] = upvals[*idx as usize].clone();
                }
                Op::Move { dst, src } => {
                    stack[base + *dst as usize] = stack[base + *src as usize].clone();
                }

                Op::Bin { dst, a, b, op } => {
                    let v = apply_bin(*op, &stack[base + *a as usize], &stack[base + *b as usize])?;
                    stack[base + *dst as usize] = v;
                }
                Op::BinImm { dst, a, imm, op } => {
                    let v = apply_bin_imm(*op, &stack[base + *a as usize], *imm)?;
                    stack[base + *dst as usize] = v;
                }
                Op::Un { dst, a, op } => {
                    let v = apply_un(*op, &stack[base + *a as usize])?;
                    stack[base + *dst as usize] = v;
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

                Op::CallFn { dst, func, base: abase, argc } => {
                    let (dst, func) = (*dst, *func as usize);
                    let (abase, argc) = (*abase as usize, *argc as usize);
                    let callee = self.functions[func].clone();
                    call!(callee, None, dst, abase, argc);
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
                        stack[base + dst as usize] = v;
                    } else {
                        let args = take_range(stack, base + abase, argc);
                        let mut v = self.dispatch_call(segs, args)?;
                        if let Some(ty) = coerce {
                            v = self.coerce_result(v, ty);
                        }
                        stack[base + dst as usize] = v;
                    }
                }
                Op::PathValue { dst, path } => {
                    let (segs, _) = &cur.paths[*path as usize];
                    stack[base + *dst as usize] = self.eval_path_value(segs)?;
                }
                Op::Method { dst, recv, name, base: abase, argc } => {
                    let (dst, recv) = (*dst, *recv as usize);
                    let (abase, argc) = (*abase as usize, *argc as usize);
                    let recv_v = stack[base + recv].clone();
                    let name = &cur.names[*name as usize];
                    let s = base + abase;
                    // The arg window holds dead temporaries, so methods may
                    // consume them in place without cloning.
                    let v = self.eval_method(recv_v, name, &mut stack[s..s + argc])?;
                    stack[base + dst as usize] = v;
                }
                Op::Ret { src } => {
                    let v = take(&mut stack[base + *src as usize]);
                    ret!(v);
                }

                Op::MakeVec { dst, base: wbase, count } => {
                    let (dst, wbase, count) = (*dst, *wbase as usize, *count as usize);
                    let items = take_range(stack, base + wbase, count);
                    stack[base + dst as usize] = Value::vec(items);
                }
                Op::MakeTuple { dst, base: wbase, count } => {
                    let (dst, wbase, count) = (*dst, *wbase as usize, *count as usize);
                    let items = take_range(stack, base + wbase, count);
                    stack[base + dst as usize] = Value::Tuple(Rc::new(RefCell::new(items)));
                }
                Op::MakeArrayRepeat { dst, val, count } => {
                    let n = match &stack[base + *count as usize] {
                        Value::Int(n) => *n as usize,
                        _ => bail!("array repeat length must be an integer"),
                    };
                    let v = stack[base + *val as usize].clone();
                    stack[base + *dst as usize] = Value::vec(std::iter::repeat_n(v, n).collect());
                }
                Op::MakeRange { dst, start, end, inclusive } => {
                    let s = int_of(&stack[base + *start as usize], "range bound")?;
                    let e = int_of(&stack[base + *end as usize], "range bound")?;
                    stack[base + *dst as usize] =
                        Value::Range { start: s, end: e, inclusive: *inclusive };
                }
                Op::IterInit { dst, src } => {
                    let src_v = stack[base + *src as usize].clone();
                    let it = match src_v {
                        // Ranges are stepped in place by ForNext, never
                        // materialized into a Vec.
                        Value::Range { .. } => src_v,
                        other => Value::vec(self.into_iter_items(other)?),
                    };
                    stack[base + *dst as usize] = it;
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
                            stack[base + *val as usize] = v;
                            self.run_pending_ctrlc()?;
                            stack[base + *idx as usize] = Value::Int(i + 1);
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
                    let mut fields = fields_with_capacity(lit.fields.len());
                    for (k, name) in lit.fields.iter().enumerate() {
                        fields.insert(name.clone(), take(&mut stack[base + wbase + k]));
                    }
                    if lit.has_rest {
                        let rest = &stack[base + wbase + lit.fields.len()];
                        if let Value::Struct { fields: bf, .. } = rest {
                            for (k, v) in bf.borrow().iter() {
                                if !fields.contains_key(k) {
                                    fields.insert(k.clone(), v.clone());
                                }
                            }
                        }
                    }
                    stack[base + dst as usize] = Value::Struct {
                        name: lit.name.clone(),
                        fields: Rc::new(RefCell::new(fields)),
                    };
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
                    stack[base + *dst as usize] =
                        Value::Closure(Rc::new(ClosureData { chunk: child_chunk, captured }));
                }

                Op::Index { dst, base: b, key } => {
                    let v = self.index(&stack[base + *b as usize], &stack[base + *key as usize])?;
                    stack[base + *dst as usize] = v;
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
                    stack[base + *dst as usize] = v;
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
                    stack[base + *dst as usize] = v;
                }
                Op::Coerce { dst, src, ty } => {
                    let v = self
                        .coerce_value(stack[base + *src as usize].clone(), &cur.casts[*ty as usize]);
                    stack[base + *dst as usize] = v;
                }

                Op::TestBind { val, pat, dst } => {
                    let info = &cur.pats[*pat as usize];
                    let value = stack[base + *val as usize].clone();
                    let binds = &info.binds;
                    let matched = {
                        let mut define = |name: &str, v: Value| {
                            if let Some((_, reg)) = binds.iter().find(|(n, _)| n == name) {
                                stack[base + *reg as usize] = v;
                            }
                        };
                        try_bind(&info.pat, &value, &mut define)
                    };
                    stack[base + *dst as usize] = Value::Bool(matched);
                }

                Op::Fmt { dst, spec } => {
                    let text = self.render_fmt(&cur, *spec, &stack[base..])?;
                    stack[base + *dst as usize] = Value::str(text);
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
                            stack[base + *dst as usize] = Value::err(Value::str(text));
                        }
                        MacroKind::Bail => {
                            ret!(Value::err(Value::str(text)));
                        }
                    }
                    if !matches!(kind, MacroKind::Anyhow) {
                        stack[base + *dst as usize] = Value::Unit;
                    }
                }
                Op::Dbg { dst, base: wbase, argc } => {
                    let (dst, wbase, argc) = (*dst, *wbase as usize, *argc as usize);
                    let mut last = Value::Unit;
                    for i in 0..argc {
                        last = stack[base + wbase + i].clone();
                        eprintln!("[dbg] {}", last.debug());
                    }
                    stack[base + dst as usize] = last;
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

fn int_of(v: &Value, what: &str) -> Result<i64> {
    match v {
        Value::Int(i) => Ok(*i),
        _ => bail!("{what} must be an integer"),
    }
}

// -- operators -------------------------------------------------------------

pub(super) fn apply_bin(op: BinKind, l: &Value, r: &Value) -> Result<Value> {
    use BinKind::*;
    Ok(match op {
        Add | Sub | Mul | Div | Rem => return arith(op, l, r),
        Eq => Value::Bool(l.eq_value(r)),
        Ne => Value::Bool(!l.eq_value(r)),
        Lt => Value::Bool(compare(l, r)? == Ordering::Less),
        Le => Value::Bool(compare(l, r)? != Ordering::Greater),
        Gt => Value::Bool(compare(l, r)? == Ordering::Greater),
        Ge => Value::Bool(compare(l, r)? != Ordering::Less),
        BitAnd => int_bin(l, r, |a, b| a & b)?,
        BitOr => int_bin(l, r, |a, b| a | b)?,
        BitXor => int_bin(l, r, |a, b| a ^ b)?,
        Shl => int_bin(l, r, |a, b| a << b)?,
        Shr => int_bin(l, r, |a, b| a >> b)?,
    })
}

/// `apply_bin` with an integer literal right operand, with a fast integer path
/// that skips building a `Value` for the literal.
fn apply_bin_imm(op: BinKind, l: &Value, imm: i64) -> Result<Value> {
    use BinKind::*;
    if let Value::Int(a) = l {
        let a = *a;
        return Ok(match op {
            Add => Value::Int(a.wrapping_add(imm)),
            Sub => Value::Int(a.wrapping_sub(imm)),
            Mul => Value::Int(a.wrapping_mul(imm)),
            Div => {
                if imm == 0 {
                    bail!("divide by zero");
                }
                Value::Int(a.wrapping_div(imm))
            }
            Rem => {
                if imm == 0 {
                    bail!("remainder by zero");
                }
                Value::Int(a.wrapping_rem(imm))
            }
            Eq => Value::Bool(a == imm),
            Ne => Value::Bool(a != imm),
            Lt => Value::Bool(a < imm),
            Le => Value::Bool(a <= imm),
            Gt => Value::Bool(a > imm),
            Ge => Value::Bool(a >= imm),
            BitAnd => Value::Int(a & imm),
            BitOr => Value::Int(a | imm),
            BitXor => Value::Int(a ^ imm),
            Shl => Value::Int(a << imm),
            Shr => Value::Int(a >> imm),
        });
    }
    apply_bin(op, l, &Value::Int(imm))
}

/// Comparison result for the fused compare-and-branch ops.
fn cmp_test(op: BinKind, l: &Value, r: &Value) -> Result<bool> {
    use BinKind::*;
    Ok(match op {
        Eq => l.eq_value(r),
        Ne => !l.eq_value(r),
        Lt => compare(l, r)? == Ordering::Less,
        Le => compare(l, r)? != Ordering::Greater,
        Gt => compare(l, r)? == Ordering::Greater,
        Ge => compare(l, r)? != Ordering::Less,
        _ => unreachable!("compare jump carries a non-comparison operator"),
    })
}

fn cmp_test_imm(op: BinKind, l: &Value, imm: i64) -> Result<bool> {
    use BinKind::*;
    if let Value::Int(a) = l {
        let a = *a;
        return Ok(match op {
            Eq => a == imm,
            Ne => a != imm,
            Lt => a < imm,
            Le => a <= imm,
            Gt => a > imm,
            Ge => a >= imm,
            _ => unreachable!("compare jump carries a non-comparison operator"),
        });
    }
    cmp_test(op, l, &Value::Int(imm))
}

fn arith(op: BinKind, l: &Value, r: &Value) -> Result<Value> {
    use BinKind::*;
    if let (Add, Value::Str(a), Value::Str(b)) = (op, l, r) {
        return Ok(Value::str(format!("{}{}", a.borrow(), b.borrow())));
    }
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => {
            let (a, b) = (*a, *b);
            Ok(Value::Int(match op {
                Add => a.wrapping_add(b),
                Sub => a.wrapping_sub(b),
                Mul => a.wrapping_mul(b),
                Div => {
                    if b == 0 {
                        bail!("divide by zero");
                    }
                    a.wrapping_div(b)
                }
                Rem => {
                    if b == 0 {
                        bail!("remainder by zero");
                    }
                    a.wrapping_rem(b)
                }
                _ => unreachable!(),
            }))
        }
        (a, b) => {
            let (x, y) = (to_float(a)?, to_float(b)?);
            Ok(Value::Float(match op {
                Add => x + y,
                Sub => x - y,
                Mul => x * y,
                Div => x / y,
                Rem => x % y,
                _ => unreachable!(),
            }))
        }
    }
}

fn int_bin(l: &Value, r: &Value, f: impl Fn(i64, i64) -> i64) -> Result<Value> {
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(f(*a, *b))),
        (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(f(*a as i64, *b as i64) != 0)),
        _ => bail!("bitwise operators need integers"),
    }
}

fn compare(l: &Value, r: &Value) -> Result<Ordering> {
    Ok(match (l, r) {
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::Float(a), Value::Float(b)) => a.partial_cmp(b).ok_or_else(|| anyhow!("cannot order NaN"))?,
        (Value::Int(a), Value::Float(b)) => {
            (*a as f64).partial_cmp(b).ok_or_else(|| anyhow!("cannot order NaN"))?
        }
        (Value::Float(a), Value::Int(b)) => {
            a.partial_cmp(&(*b as f64)).ok_or_else(|| anyhow!("cannot order NaN"))?
        }
        (Value::Str(a), Value::Str(b)) => a.borrow().cmp(&b.borrow()),
        (Value::Char(a), Value::Char(b)) => a.cmp(b),
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        (a, b) => bail!("cannot compare {} and {}", a.type_name(), b.type_name()),
    })
}

fn to_float(v: &Value) -> Result<f64> {
    match v {
        Value::Int(i) => Ok(*i as f64),
        Value::Float(f) => Ok(*f),
        other => bail!("expected a number, got {}", other.type_name()),
    }
}

fn apply_un(op: UnKind, v: &Value) -> Result<Value> {
    Ok(match (op, v) {
        (UnKind::Neg, Value::Int(i)) => Value::Int(-*i),
        (UnKind::Neg, Value::Float(f)) => Value::Float(-*f),
        (UnKind::Not, Value::Bool(b)) => Value::Bool(!*b),
        (UnKind::Not, Value::Int(i)) => Value::Int(!*i),
        (op, v) => bail!("cannot apply {:?} to {}", op, v.type_name()),
    })
}

// -- patterns --------------------------------------------------------------

/// Match `pat` against `val`, calling `define` for each bound name. Returns
/// false without fully binding when the pattern does not match.
pub(super) fn try_bind(pat: &Pat, val: &Value, define: &mut dyn FnMut(&str, Value)) -> bool {
    match pat {
        Pat::Wild(_) | Pat::Rest(_) => true,
        Pat::Ident(id) => {
            if let Some(sub) = &id.subpat
                && !try_bind(&sub.1, val, define)
            {
                return false;
            }
            define(&id.ident.to_string(), val.clone());
            true
        }
        Pat::Lit(lit) => match lit_value(&lit.lit) {
            Some(expected) => expected.eq_value(val),
            None => false,
        },
        Pat::Paren(p) => try_bind(&p.pat, val, define),
        Pat::Reference(r) => try_bind(&r.pat, val, define),
        Pat::Type(t) => try_bind(&t.pat, val, define),
        Pat::Tuple(t) => match val {
            Value::Tuple(items) => bind_seq(t.elems.iter(), &items.borrow(), define),
            _ => false,
        },
        Pat::TupleStruct(ts) => {
            let name = ts.path.segments.last().map(|s| s.ident.to_string());
            match val {
                Value::Enum { variant, data, .. } => {
                    name.as_deref() == Some(&**variant)
                        && bind_seq(ts.elems.iter(), data, define)
                }
                Value::Struct { fields, .. } => {
                    let vals: Vec<Value> = fields.borrow().values().cloned().collect();
                    bind_seq(ts.elems.iter(), &vals, define)
                }
                _ => false,
            }
        }
        Pat::Path(p) => {
            let name = p.path.segments.last().map(|s| s.ident.to_string());
            match val {
                Value::Enum { variant, .. } => name.as_deref() == Some(&**variant),
                _ => false,
            }
        }
        Pat::Struct(s) => {
            let name = s.path.segments.last().map(|s| s.ident.to_string());
            let fields = match val {
                Value::Struct { name: n, fields } => {
                    if let Some(pn) = &name
                        && pn.as_str() != &**n
                    {
                        return false;
                    }
                    fields.borrow()
                }
                _ => return false,
            };
            for f in &s.fields {
                let key = match &f.member {
                    syn::Member::Named(n) => n.to_string(),
                    syn::Member::Unnamed(i) => i.index.to_string(),
                };
                match fields.get(key.as_str()) {
                    Some(v) => {
                        if !try_bind(&f.pat, v, define) {
                            return false;
                        }
                    }
                    None => return false,
                }
            }
            true
        }
        Pat::Or(or) => or.cases.iter().any(|c| try_bind(c, val, define)),
        Pat::Slice(s) => match val {
            Value::Vec(items) => bind_seq(s.elems.iter(), &items.borrow(), define),
            _ => false,
        },
        _ => false,
    }
}

fn bind_seq<'a>(
    pats: impl Iterator<Item = &'a Pat>,
    vals: &[Value],
    define: &mut dyn FnMut(&str, Value),
) -> bool {
    let pats: Vec<&Pat> = pats.collect();
    if pats.iter().any(|p| matches!(p, Pat::Rest(_))) {
        let head_len = pats.iter().take_while(|p| !matches!(p, Pat::Rest(_))).count();
        for (p, v) in pats.iter().take(head_len).zip(vals.iter()) {
            if !try_bind(p, v, define) {
                return false;
            }
        }
        let tail: Vec<&&Pat> = pats.iter().skip(head_len + 1).collect();
        for (p, v) in tail.iter().zip(vals.iter().rev()) {
            if !try_bind(p, v, define) {
                return false;
            }
        }
        return true;
    }
    if pats.len() != vals.len() {
        return false;
    }
    pats.iter().zip(vals.iter()).all(|(p, v)| try_bind(p, v, define))
}

pub(super) fn lit_value(lit: &Lit) -> Option<Value> {
    Some(match lit {
        Lit::Int(i) => Value::Int(i.base10_parse::<i64>().ok()?),
        Lit::Float(f) => Value::Float(f.base10_parse::<f64>().ok()?),
        Lit::Bool(b) => Value::Bool(b.value),
        Lit::Str(s) => Value::str(s.value()),
        Lit::Char(c) => Value::Char(c.value()),
        Lit::Byte(b) => Value::Int(b.value() as i64),
        _ => return None,
    })
}
