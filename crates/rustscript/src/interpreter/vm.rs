//! The register machine. Executes a compiled `Chunk` against a flat register
//! frame. Arithmetic, comparison, control flow, and calls to user functions run
//! entirely here. Anything else, methods and std or crate bridges, is delegated
//! to the existing dispatch on `Interp` with already evaluated values.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

use anyhow::{Result, anyhow, bail};
use syn::{Lit, Pat};

use super::bytecode::{BinKind, CapSource, Chunk, MacroKind, Op, UnKind};
use super::value::{ClosureData, Fields, Value};
use super::Interp;

/// A signal a chunk can raise that unwinds past the instruction loop.
enum Signal {
    /// `return x`, `?` on an error, or `bail!`, the chunk's result.
    Return(Value),
}

thread_local! {
    /// Recycled register frames, so a hot recursive call does not allocate a
    /// fresh `Vec` every time.
    static FRAME_POOL: RefCell<Vec<Vec<Value>>> = const { RefCell::new(Vec::new()) };
}

fn take_frame(size: usize) -> Vec<Value> {
    let mut f = FRAME_POOL.with(|p| p.borrow_mut().pop()).unwrap_or_default();
    f.clear();
    f.resize(size, Value::Unit);
    f
}

fn recycle_frame(mut f: Vec<Value>) {
    f.clear();
    FRAME_POOL.with(|p| {
        let mut p = p.borrow_mut();
        if p.len() < 256 {
            p.push(f);
        }
    });
}

impl Interp {
    /// Run a compiled function or closure body with a checked argument count.
    pub(super) fn run_chunk(
        &self,
        chunk: &Chunk,
        args: &[Value],
        upvalues: &[Value],
    ) -> Result<Value> {
        if args.len() != chunk.num_params {
            bail!("`{}` expects {} args but got {}", chunk.name, chunk.num_params, args.len());
        }
        let mut regs = take_frame(chunk.num_regs.max(chunk.num_params));
        for (i, a) in args.iter().enumerate() {
            regs[i] = a.clone();
        }
        let result = self.exec(chunk, &mut regs, upvalues);
        recycle_frame(regs);
        result.map(|Signal::Return(v)| v)
    }

    /// Call a chunk, copying arguments straight out of the caller's registers so
    /// no intermediate `Vec` is built on the hot path.
    fn call_from(
        &self,
        chunk: &Chunk,
        src: &[Value],
        arg_base: usize,
        argc: usize,
        upvalues: &[Value],
    ) -> Result<Value> {
        let mut regs = take_frame(chunk.num_regs.max(chunk.num_params));
        for i in 0..argc {
            regs[i] = src[arg_base + i].clone();
        }
        let result = self.exec(chunk, &mut regs, upvalues);
        recycle_frame(regs);
        result.map(|Signal::Return(v)| v)
    }

    fn exec(&self, chunk: &Chunk, regs: &mut [Value], upvalues: &[Value]) -> Result<Signal> {
        let code = &chunk.code;
        let mut ip = 0usize;
        while ip < code.len() {
            match &code[ip] {
                Op::LoadConst { dst, k } => regs[*dst as usize] = chunk.consts[*k as usize].clone(),
                Op::LoadInt { dst, v } => regs[*dst as usize] = Value::Int(*v),
                Op::LoadBool { dst, v } => regs[*dst as usize] = Value::Bool(*v),
                Op::LoadUnit { dst } => regs[*dst as usize] = Value::Unit,
                Op::LoadUpvalue { dst, idx } => regs[*dst as usize] = upvalues[*idx as usize].clone(),
                Op::Move { dst, src } => regs[*dst as usize] = regs[*src as usize].clone(),

                Op::Bin { dst, a, b, op } => {
                    let v = apply_bin(*op, &regs[*a as usize], &regs[*b as usize])?;
                    regs[*dst as usize] = v;
                }
                Op::Un { dst, a, op } => {
                    let v = apply_un(*op, &regs[*a as usize])?;
                    regs[*dst as usize] = v;
                }

                Op::Jump { to } => {
                    if (*to as usize) <= ip {
                        self.run_pending_ctrlc()?;
                    }
                    ip = *to as usize;
                    continue;
                }
                Op::JumpIfFalse { cond, to } => {
                    if !regs[*cond as usize].is_truthy() {
                        ip = *to as usize;
                        continue;
                    }
                }
                Op::JumpIfTrue { cond, to } => {
                    if regs[*cond as usize].is_truthy() {
                        ip = *to as usize;
                        continue;
                    }
                }

                Op::CallFn { dst, func, base, argc } => {
                    // Borrow the callee, no refcount traffic on the hot path.
                    let f = &*self.functions[*func as usize];
                    let v = self.call_from(f, regs, *base as usize, *argc as usize, &[])?;
                    regs[*dst as usize] = v;
                }
                Op::CallValue { dst, callee, base, argc } => {
                    let v = match &regs[*callee as usize] {
                        Value::Closure(clo) => {
                            let clo = clo.clone();
                            self.call_from(&clo.chunk, regs, *base as usize, *argc as usize, &clo.captured)?
                        }
                        other => bail!("cannot call {}", other.type_name()),
                    };
                    regs[*dst as usize] = v;
                }
                Op::CallPath { dst, path, base, argc } => {
                    let (segs, coerce) = &chunk.paths[*path as usize];
                    if let Some(v) = self.internal_path(segs, regs, *base, *argc)? {
                        regs[*dst as usize] = v;
                    } else {
                        let args = collect(regs, *base, *argc);
                        let mut v = self.dispatch_call(segs, args)?;
                        if let Some(ty) = coerce {
                            v = self.coerce_result(v, ty);
                        }
                        regs[*dst as usize] = v;
                    }
                }
                Op::PathValue { dst, path } => {
                    let (segs, _) = &chunk.paths[*path as usize];
                    regs[*dst as usize] = self.eval_path_value(segs)?;
                }
                Op::Method { dst, recv, name, base, argc } => {
                    let recv_v = regs[*recv as usize].clone();
                    let args = collect(regs, *base, *argc);
                    let v = self.eval_method(recv_v, &chunk.names[*name as usize], args)?;
                    regs[*dst as usize] = v;
                }
                Op::Ret { src } => return Ok(Signal::Return(regs[*src as usize].clone())),

                Op::MakeVec { dst, base, count } => {
                    let items = collect(regs, *base, *count);
                    regs[*dst as usize] = Value::vec(items);
                }
                Op::MakeTuple { dst, base, count } => {
                    let items = collect(regs, *base, *count);
                    regs[*dst as usize] = Value::Tuple(Rc::new(RefCell::new(items)));
                }
                Op::MakeArrayRepeat { dst, val, count } => {
                    let n = match &regs[*count as usize] {
                        Value::Int(n) => *n as usize,
                        _ => bail!("array repeat length must be an integer"),
                    };
                    let v = regs[*val as usize].clone();
                    regs[*dst as usize] = Value::vec(std::iter::repeat_n(v, n).collect());
                }
                Op::MakeRange { dst, start, end, inclusive } => {
                    let s = int_of(&regs[*start as usize], "range bound")?;
                    let e = int_of(&regs[*end as usize], "range bound")?;
                    regs[*dst as usize] = Value::Range { start: s, end: e, inclusive: *inclusive };
                }
                Op::IterInit { dst, src } => {
                    let items = self.into_iter_items(regs[*src as usize].clone())?;
                    regs[*dst as usize] = Value::vec(items);
                }
                Op::ForNext { iter, idx, val, to } => {
                    let i = match &regs[*idx as usize] {
                        Value::Int(i) => *i as usize,
                        _ => unreachable!("for index is an integer"),
                    };
                    let item = match &regs[*iter as usize] {
                        Value::Vec(items) => items.borrow().get(i).cloned(),
                        _ => None,
                    };
                    match item {
                        Some(v) => {
                            regs[*val as usize] = v;
                            self.run_pending_ctrlc()?;
                            regs[*idx as usize] = Value::Int(i as i64 + 1);
                        }
                        None => {
                            ip = *to as usize;
                            continue;
                        }
                    }
                }
                Op::MakeStruct { dst, info, base } => {
                    let lit = &chunk.struct_lits[*info as usize];
                    let mut fields = Fields::new();
                    for (k, name) in lit.fields.iter().enumerate() {
                        fields.insert(name.clone(), regs[*base as usize + k].clone());
                    }
                    if lit.has_rest {
                        let rest = &regs[*base as usize + lit.fields.len()];
                        if let Value::Struct { fields: bf, .. } = rest {
                            for (k, v) in bf.borrow().iter() {
                                if !fields.contains_key(k) {
                                    fields.insert(k.clone(), v.clone());
                                }
                            }
                        }
                    }
                    regs[*dst as usize] = Value::Struct {
                        name: lit.name.clone(),
                        fields: Rc::new(RefCell::new(fields)),
                    };
                }
                Op::MakeClosure { dst, child } => {
                    let child_chunk = chunk.children[*child as usize].clone();
                    let caps = &chunk.child_caps[*child as usize];
                    let captured: Vec<Value> = caps
                        .iter()
                        .map(|c| match c {
                            CapSource::Local(reg) => regs[*reg as usize].clone(),
                            CapSource::Upvalue(idx) => upvalues[*idx as usize].clone(),
                        })
                        .collect();
                    regs[*dst as usize] =
                        Value::Closure(Rc::new(ClosureData { chunk: child_chunk, captured }));
                }

                Op::Index { dst, base, key } => {
                    let v = self.index(&regs[*base as usize], &regs[*key as usize])?;
                    regs[*dst as usize] = v;
                }
                Op::SetIndex { base, key, val } => {
                    self.set_index(&regs[*base as usize], &regs[*key as usize], regs[*val as usize].clone())?;
                }
                Op::GetField { dst, base, member } => {
                    let v = self.get_field(&regs[*base as usize], &chunk.members[*member as usize])?;
                    regs[*dst as usize] = v;
                }
                Op::SetField { base, member, val } => {
                    self.set_field(&regs[*base as usize], &chunk.members[*member as usize], regs[*val as usize].clone())?;
                }

                Op::Try { dst, src } => match self.eval_try(regs[*src as usize].clone())? {
                    Ok(v) => regs[*dst as usize] = v,
                    Err(early) => return Ok(Signal::Return(early)),
                },
                Op::Cast { dst, src, ty } => {
                    let v = self.eval_cast(regs[*src as usize].clone(), &chunk.casts[*ty as usize])?;
                    regs[*dst as usize] = v;
                }
                Op::Coerce { dst, src, ty } => {
                    let v = self.coerce_value(regs[*src as usize].clone(), &chunk.casts[*ty as usize]);
                    regs[*dst as usize] = v;
                }

                Op::TestBind { val, pat, dst } => {
                    let info = &chunk.pats[*pat as usize];
                    let value = regs[*val as usize].clone();
                    let binds = &info.binds;
                    let matched = {
                        let mut define = |name: &str, v: Value| {
                            if let Some((_, reg)) = binds.iter().find(|(n, _)| n == name) {
                                regs[*reg as usize] = v;
                            }
                        };
                        try_bind(&info.pat, &value, &mut define)
                    };
                    regs[*dst as usize] = Value::Bool(matched);
                }

                Op::Fmt { dst, spec } => {
                    let text = self.render_fmt(chunk, *spec, regs)?;
                    regs[*dst as usize] = Value::str(text);
                }
                Op::MacroCall { kind, dst, spec } => {
                    let text = self.render_fmt(chunk, *spec, regs)?;
                    match kind {
                        MacroKind::Println => println!("{text}"),
                        MacroKind::Print => print!("{text}"),
                        MacroKind::Eprintln => eprintln!("{text}"),
                        MacroKind::Eprint => eprint!("{text}"),
                        MacroKind::Panic => bail!("panicked: {text}"),
                        MacroKind::Anyhow => {
                            regs[*dst as usize] = Value::err(Value::str(text));
                        }
                        MacroKind::Bail => {
                            return Ok(Signal::Return(Value::err(Value::str(text))));
                        }
                    }
                    if !matches!(kind, MacroKind::Anyhow) {
                        regs[*dst as usize] = Value::Unit;
                    }
                }
                Op::Dbg { dst, base, argc } => {
                    let mut last = Value::Unit;
                    for i in 0..*argc {
                        last = regs[*base as usize + i as usize].clone();
                        eprintln!("[dbg] {}", last.debug());
                    }
                    regs[*dst as usize] = last;
                }
            }
            ip += 1;
        }
        Ok(Signal::Return(Value::Unit))
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
        base: u16,
        argc: u16,
    ) -> Result<Option<Value>> {
        let head = segs.first().map(|s| s.as_str()).unwrap_or("");
        match head {
            "::unreachable_match" => bail!("no match arm matched the value"),
            "::assert_failed" => bail!("assertion failed"),
            "::ensure_fail" => {
                let msg = if argc > 0 {
                    regs[base as usize].display()
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

fn collect(regs: &[Value], base: u16, count: u16) -> Vec<Value> {
    (0..count as usize).map(|i| regs[base as usize + i].clone()).collect()
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
                    name.as_deref() == Some(variant.as_str())
                        && bind_seq(ts.elems.iter(), &data.borrow(), define)
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
                Value::Enum { variant, .. } => name.as_deref() == Some(variant.as_str()),
                _ => false,
            }
        }
        Pat::Struct(s) => {
            let name = s.path.segments.last().map(|s| s.ident.to_string());
            let fields = match val {
                Value::Struct { name: n, fields } => {
                    if let Some(pn) = &name
                        && pn != n
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
                match fields.get(&key) {
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

