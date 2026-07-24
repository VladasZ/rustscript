//! The parallel register machine. It mirrors `vm.rs` but runs `PChunk` over
//! `PValue`, so a task can run on any worker thread. The hot inline fast paths
//! of the fast VM are dropped here for clarity; the parallel engine optimizes
//! for correctness and real concurrency, not single thread microspeed.

use std::collections::HashMap;
use std::mem::{replace, take};
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use parking_lot::Mutex;
use tokio::runtime::Handle;

use super::bytecode::{BuiltinId, CapSource, MacroKind, MethodName, Op};
use super::numeric::{float_to_int, truncate};
use super::pchunk::{PChunk, PMember};
use super::pnative::PNative;
use super::pops::{
    self, apply_bin, apply_bin_imm, apply_un, cmp_test, cmp_test_imm, int_of, try_bind,
};
use super::pvalue::{PClosure, PStructShape, PUpvalue, PValue};
use super::typeir::{CastIr, TypeIr};
use super::vm_support::trace_error;

const MAX_CALL_DEPTH: usize = 100_000;

fn swap_option<T>(current: &mut Option<T>, next: Option<T>) -> Option<T> {
    match next {
        Some(value) => current.replace(value),
        None => current.take(),
    }
}

/// One backtrace entry: the function, its file, and the line of the op at
/// `ip`. The fast engine has its twin in `vm.rs`.
fn frame_line(chunk: &PChunk, ip: usize) -> (String, String, u32) {
    let line = chunk.lines.get(ip).copied().unwrap_or(0);
    (chunk.name.clone(), chunk.file.to_string(), line)
}

/// A module level const or static: converted once, evaluated on first read.
/// Each slot has its own lock so tasks on different threads can read globals.
pub enum PGlobalSlot {
    Todo(Arc<PChunk>),
    Busy,
    Ready(PValue),
}

/// The compiled program plus the runtime handle, shared across worker threads.
pub struct PInterp {
    pub functions: Vec<Arc<PChunk>>,
    pub fn_index: HashMap<String, u32>,
    pub methods: HashMap<(String, String), Arc<PChunk>>,
    pub globals: Vec<Mutex<PGlobalSlot>>,
    /// User struct layouts precomputed at load, for coercion and typed json.
    pub structs: super::pjson::PStructs,
    pub rt: Handle,
}

/// A binding of a generic parameter name to the lowered concrete type a
/// caller passed by turbofish, the parallel twin of `TypeEnv` in vm.rs.
pub(super) type PTypeEnv = Arc<[(Arc<str>, TypeIr)]>;

fn empty_ptype_env() -> PTypeEnv {
    Arc::from(Vec::new())
}

struct Frame {
    chunk: Arc<PChunk>,
    closure: Option<Arc<PClosure>>,
    ip: usize,
    base: usize,
    dst: u16,
    /// The caller's arg window, so the callee's final parameter values can be
    /// handed back on return for `&mut` argument writebacks.
    abase: u16,
    argc: u16,
    type_env: PTypeEnv,
}

impl PInterp {
    pub fn run_chunk(
        self: &Arc<Self>,
        chunk: &Arc<PChunk>,
        args: &[PValue],
        upvalues: &[PUpvalue],
    ) -> Result<PValue> {
        if args.len() != chunk.num_params {
            bail!(
                "`{}` expects {} args but got {}",
                chunk.name,
                chunk.num_params,
                args.len()
            );
        }
        let mut stack = vec![PValue::Unit; chunk.num_regs.max(chunk.num_params)];
        for (i, a) in args.iter().enumerate() {
            stack[i] = a.clone();
        }
        self.exec(chunk, &mut stack, upvalues)
    }

    fn exec(
        self: &Arc<Self>,
        entry: &Arc<PChunk>,
        stack: &mut Vec<PValue>,
        entry_upvalues: &[PUpvalue],
    ) -> Result<PValue> {
        let mut frames: Vec<Frame> = Vec::new();
        let mut local_cells: HashMap<usize, Arc<Mutex<PValue>>> = HashMap::new();
        let mut cur = entry.clone();
        let mut cur_clo: Option<Arc<PClosure>> = None;
        let mut cur_tenv: PTypeEnv = empty_ptype_env();
        let mut base = 0usize;
        let mut ip = 0usize;

        // The dispatch runs inside one immediately called closure so an error
        // can be annotated with the script call chain still held in `frames`
        // and the failing op still addressed by `cur` and `ip`. The closure
        // runs exactly once, so the hot loop itself is unchanged.
        let result = (|| -> Result<PValue> {
            macro_rules! ret {
                ($v:expr) => {{
                    let v = $v;
                    match frames.pop() {
                        None => return Ok(v),
                        Some(f) => {
                            let callee_base = base;
                            let callee_end = callee_base + cur.num_regs;
                            local_cells
                                .retain(|slot, _| *slot < callee_base || *slot >= callee_end);
                            cur = f.chunk;
                            cur_clo = f.closure;
                            cur_tenv = f.type_env;
                            ip = f.ip;
                            // The callee's final parameter values go back into the
                            // caller's arg window, where a `&mut` argument
                            // writeback emitted by the compiler picks them up.
                            base = f.base;
                            for i in 0..f.argc as usize {
                                stack[base + f.abase as usize + i] =
                                    take(&mut stack[callee_base + i]);
                            }
                            stack[base + f.dst as usize] = v;
                            continue;
                        }
                    }
                }};
            }

            macro_rules! call {
                ($chunk:expr, $clo:expr, $dst:expr, $abase:expr, $argc:expr) => {
                    call!($chunk, $clo, $dst, $abase, $argc, empty_ptype_env())
                };
                ($chunk:expr, $clo:expr, $dst:expr, $abase:expr, $argc:expr, $tenv:expr) => {{
                    let callee: Arc<PChunk> = $chunk;
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
                        stack.resize(need, PValue::Unit);
                    }
                    for i in 0..$argc {
                        stack[nbase + i] = take(&mut stack[base + $abase + i]);
                    }
                    for slot in &mut stack[nbase + $argc..need] {
                        *slot = PValue::Unit;
                    }
                    frames.push(Frame {
                        chunk: replace(&mut cur, callee),
                        closure: swap_option(&mut cur_clo, $clo),
                        ip: ip + 1,
                        base,
                        dst: $dst,
                        abase: $abase as u16,
                        argc: $argc as u16,
                        type_env: replace(&mut cur_tenv, $tenv),
                    });
                    base = nbase;
                    ip = 0;
                    continue;
                }};
            }

            loop {
                if ip >= cur.code.len() {
                    ret!(PValue::Unit);
                }
                match &cur.code[ip] {
                    Op::LoadConst { dst, k } => {
                        stack[base + *dst as usize] = PValue::from_const(&cur.consts[*k as usize]);
                    }
                    Op::LoadInt { dst, v } => stack[base + *dst as usize] = PValue::Int(*v),
                    Op::LoadIntW { dst, v, w } => {
                        stack[base + *dst as usize] = PValue::IntW(*v, *w);
                    }
                    Op::LoadBool { dst, v } => stack[base + *dst as usize] = PValue::Bool(*v),
                    Op::LoadUnit { dst } => stack[base + *dst as usize] = PValue::Unit,
                    Op::LoadGlobal { dst, idx } => {
                        let v = self.global(*idx as usize)?;
                        stack[base + *dst as usize] = v;
                    }
                    Op::LoadUpvalue { dst, idx } => {
                        let upvals: &[PUpvalue] = match &cur_clo {
                            Some(c) => &c.captured,
                            None => entry_upvalues,
                        };
                        stack[base + *dst as usize] = upvals[*idx as usize].get();
                    }
                    Op::LoadCell { dst, cell } => {
                        let slot = base + *cell as usize;
                        let Some(value) = local_cells.get(&slot) else {
                            bail!("missing mutable capture cell");
                        };
                        stack[base + *dst as usize] = value.lock().clone();
                    }
                    Op::StoreCell { cell, src } => {
                        let slot = base + *cell as usize;
                        let Some(value) = local_cells.get(&slot) else {
                            bail!("missing mutable capture cell");
                        };
                        *value.lock() = stack[base + *src as usize].clone();
                    }
                    Op::StoreUpvalue { idx, src } => {
                        let upvalues: &[PUpvalue] = match &cur_clo {
                            Some(closure) => &closure.captured,
                            None => entry_upvalues,
                        };
                        if !upvalues[*idx as usize].set(stack[base + *src as usize].clone()) {
                            bail!("cannot assign to immutable capture");
                        }
                    }
                    Op::Move { dst, src } => {
                        stack[base + *dst as usize] = stack[base + *src as usize].clone();
                    }
                    Op::Bin { dst, a, b, op } => {
                        let v =
                            apply_bin(*op, &stack[base + *a as usize], &stack[base + *b as usize])?;
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
                        ip = *to as usize;
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
                    Op::CallFn {
                        dst,
                        func,
                        base: abase,
                        argc,
                        targ,
                    } => {
                        let (dst, func, abase, argc) =
                            (*dst, *func as usize, *abase as usize, *argc as usize);
                        let callee = self.functions[func].clone();
                        // Bind the call's turbofish type args to the callee's
                        // generic parameters, mirroring the fast VM.
                        let tenv: PTypeEnv = if *targ != u32::MAX {
                            let targs = &cur.call_type_args[*targ as usize];
                            callee
                                .generics
                                .iter()
                                .zip(targs.iter())
                                .map(|(name, ty)| (name.clone(), ty.clone()))
                                .collect()
                        } else {
                            empty_ptype_env()
                        };
                        call!(callee, None, dst, abase, argc, tenv);
                    }
                    Op::CallValue {
                        dst,
                        callee,
                        base: abase,
                        argc,
                    } => {
                        let (dst, callee_reg, abase, argc) =
                            (*dst, *callee as usize, *abase as usize, *argc as usize);
                        let clo = match &stack[base + callee_reg] {
                            PValue::Closure(clo) => clo.clone(),
                            other => bail!("cannot call {}", other.type_name()),
                        };
                        let chunk = clo.chunk.clone();
                        call!(chunk, Some(clo), dst, abase, argc);
                    }
                    Op::CallPath {
                        dst,
                        path,
                        base: abase,
                        argc,
                    } => {
                        let (abase, argc) = (*abase as usize, *argc as usize);
                        let (segs, coerce) = &cur.paths[*path as usize];
                        let args = take_range(stack, base + abase, argc);
                        // Typed json parses straight into the target structs,
                        // mirroring the fast VM's `serde_json::from_str` path.
                        if let Some(ty) = coerce
                            && segs.len() >= 2
                            && segs[segs.len() - 2] == "serde_json"
                            && segs[segs.len() - 1] == "from_str"
                        {
                            let v = self.typed_from_str(&args, ty, &cur_tenv)?;
                            stack[base + *dst as usize] = v;
                            ip += 1;
                            continue;
                        }
                        let mut v = self.dispatch_call(segs, args)?;
                        if let Some(ty) = coerce {
                            v = self.coerce_result(v, ty);
                        }
                        stack[base + *dst as usize] = v;
                    }
                    Op::PathValue { dst, path } => {
                        let (segs, _) = &cur.paths[*path as usize];
                        stack[base + *dst as usize] = self.eval_path_value(segs)?;
                    }
                    Op::Method {
                        dst,
                        recv,
                        name,
                        base: abase,
                        argc,
                    } => {
                        let (recv, abase, argc) = (*recv as usize, *abase as usize, *argc as usize);
                        let name = &cur.names[*name as usize];
                        // A string is an immutable `Arc<str>`, so a push has to
                        // rewrite the receiver register itself. The normal path
                        // hands the method a clone and the change would be lost.
                        // `clone_from` replaces the receiver outright, so it has
                        // to write the register rather than a copy of it.
                        if name.id == BuiltinId::CloneFrom {
                            let src = stack[base + abase..base + abase + argc]
                                .first()
                                .cloned()
                                .unwrap_or(PValue::Unit);
                            stack[base + recv] = src;
                            if *dst != u16::MAX {
                                stack[base + *dst as usize] = PValue::Unit;
                            }
                            ip += 1;
                            continue;
                        }
                        if matches!(name.id, BuiltinId::Push | BuiltinId::PushStr)
                            && let PValue::Str(s) = &stack[base + recv]
                        {
                            let mut out = s.to_string();
                            match (&name.id, stack[base + abase..base + abase + argc].first()) {
                                (BuiltinId::Push, Some(PValue::Char(c))) => out.push(*c),
                                (BuiltinId::PushStr, Some(arg)) => out.push_str(&arg.display()),
                                _ => {}
                            }
                            stack[base + recv] = PValue::str(out);
                            if *dst != u16::MAX {
                                stack[base + *dst as usize] = PValue::Unit;
                            }
                            ip += 1;
                            continue;
                        }
                        let recv_v = stack[base + recv].clone();
                        let mut margs = take_range(stack, base + abase, argc);
                        let v = self.eval_method(&recv_v, name, &mut margs)?;
                        if *dst != u16::MAX {
                            stack[base + *dst as usize] = v;
                        }
                    }
                    Op::GetOrDefault {
                        dst,
                        recv,
                        key,
                        default,
                    } => {
                        let recv_v = stack[base + *recv as usize].clone();
                        let key_v = stack[base + *key as usize].clone();
                        let get = MethodName {
                            text: "get".into(),
                            id: super::bytecode::BuiltinId::Get,
                        };
                        let opt = self.eval_method(&recv_v, &get, &mut [key_v])?;
                        let v = match opt {
                            PValue::Enum { variant, data, .. } if &*variant == "Some" => {
                                data.first().cloned().unwrap_or(PValue::Unit)
                            }
                            _ => stack[base + *default as usize].clone(),
                        };
                        stack[base + *dst as usize] = v;
                    }
                    Op::Ret { src } => {
                        let v = take(&mut stack[base + *src as usize]);
                        ret!(v);
                    }
                    Op::MakeVec {
                        dst,
                        base: wbase,
                        count,
                    } => {
                        let items = take_range(stack, base + *wbase as usize, *count as usize);
                        stack[base + *dst as usize] = PValue::vec(items);
                    }
                    Op::MakeTuple {
                        dst,
                        base: wbase,
                        count,
                    } => {
                        let items = take_range(stack, base + *wbase as usize, *count as usize);
                        stack[base + *dst as usize] = PValue::tuple(items);
                    }
                    Op::MakeArrayRepeat { dst, val, count } => {
                        let n = match &stack[base + *count as usize] {
                            PValue::Int(n) => *n as usize,
                            _ => bail!("array repeat length must be an integer"),
                        };
                        let v = stack[base + *val as usize].clone();
                        stack[base + *dst as usize] =
                            PValue::vec(std::iter::repeat_n(v, n).collect());
                    }
                    Op::MakeRange {
                        dst,
                        start,
                        end,
                        inclusive,
                    } => {
                        let s = int_of(&stack[base + *start as usize])?;
                        let e = int_of(&stack[base + *end as usize])?;
                        stack[base + *dst as usize] = PValue::Range {
                            start: s,
                            end: e,
                            inclusive: *inclusive,
                        };
                    }
                    Op::IterInit { dst, src } => {
                        let src_v = stack[base + *src as usize].clone();
                        let it = match src_v {
                            // A range and a live line iterator stay lazy, so a loop
                            // over a child's pipe streams instead of buffering the
                            // whole output before the first line runs.
                            PValue::Range { .. } | PValue::Native(_) => src_v,
                            other => PValue::vec(self.iter_items(other)?),
                        };
                        stack[base + *dst as usize] = it;
                    }
                    Op::ForNext { iter, idx, val, to } => {
                        let i = match &stack[base + *idx as usize] {
                            PValue::Int(i) => *i,
                            _ => unreachable!("for index is an integer"),
                        };
                        let item = match &stack[base + *iter as usize] {
                            PValue::Vec(items) => items.lock().get(i as usize).cloned(),
                            PValue::Range {
                                start,
                                end,
                                inclusive,
                            } => {
                                let n = start + i;
                                let done = if *inclusive { n > *end } else { n >= *end };
                                if done { None } else { Some(PValue::Int(n)) }
                            }
                            PValue::Native(h) => match &mut *h.lock() {
                                PNative::Lines(lines) => match lines.next() {
                                    Some(Ok(line)) => Some(PValue::ok(PValue::str(line))),
                                    Some(Err(e)) => Some(PValue::err(PValue::str(e.to_string()))),
                                    None => None,
                                },
                                other => bail!("cannot iterate a {}", other.type_name()),
                            },
                            _ => None,
                        };
                        match item {
                            Some(v) => {
                                stack[base + *val as usize] = v;
                                stack[base + *idx as usize] = PValue::Int(i + 1);
                            }
                            None => {
                                ip = *to as usize;
                                continue;
                            }
                        }
                    }
                    Op::MakeStruct {
                        dst,
                        info,
                        base: wbase,
                    } => {
                        let wbase = *wbase as usize;
                        let lit = &cur.struct_lits[*info as usize];
                        let written = lit.shape.fields.len();
                        let mut values: Vec<PValue> = (0..written)
                            .map(|k| take(&mut stack[base + wbase + k]))
                            .collect();
                        let v = if lit.has_rest {
                            let rest = stack[base + wbase + written].clone();
                            let mut fields = lit.shape.fields.clone();
                            let mut renames = lit.shape.renames.clone();
                            if let PValue::Struct(r) = rest {
                                let rvals = r.values.lock();
                                for (slot, (k, v)) in
                                    r.shape.fields.iter().zip(rvals.iter()).enumerate()
                                {
                                    if lit.shape.slot(k).is_none() {
                                        fields.push(k.clone());
                                        values.push(v.clone());
                                        if !renames.is_empty() {
                                            renames
                                                .push(r.shape.renames.get(slot).cloned().flatten());
                                        }
                                    }
                                }
                            }
                            let shape = Arc::new(PStructShape {
                                name: lit.shape.name.clone(),
                                fields,
                                renames,
                            });
                            PValue::structure(shape, values)
                        } else {
                            PValue::structure(lit.shape.clone(), values)
                        };
                        stack[base + *dst as usize] = v;
                    }
                    Op::MakeEnum {
                        dst,
                        info,
                        base: wbase,
                        count,
                    } => {
                        let variant = &cur.enum_variants[*info as usize];
                        let data =
                            take_range(stack, base + *wbase as usize, *count as usize).into();
                        stack[base + *dst as usize] = PValue::Enum {
                            enum_name: variant.enum_name.clone(),
                            variant: variant.variant.clone(),
                            data,
                        };
                    }
                    Op::LoadEnum { dst, info } => {
                        let variant = &cur.enum_variants[*info as usize];
                        stack[base + *dst as usize] = PValue::Enum {
                            enum_name: variant.enum_name.clone(),
                            variant: variant.variant.clone(),
                            data: Vec::new().into(),
                        };
                    }
                    Op::MakeClosure { dst, child } => {
                        let clo = Self::make_closure(
                            &cur,
                            *child,
                            stack,
                            base,
                            &cur_clo,
                            entry_upvalues,
                            &mut local_cells,
                        );
                        stack[base + *dst as usize] = PValue::Closure(clo);
                    }
                    Op::Index { dst, base: b, key } => {
                        let v =
                            pops::index(&stack[base + *b as usize], &stack[base + *key as usize])?;
                        stack[base + *dst as usize] = v;
                    }
                    Op::SetIndex { base: b, key, val } => {
                        pops::set_index(
                            &stack[base + *b as usize],
                            &stack[base + *key as usize],
                            stack[base + *val as usize].clone(),
                        )?;
                    }
                    Op::Deref { dst, src } => {
                        stack[base + *dst as usize] = match &stack[base + *src as usize] {
                            PValue::Ref(reference) => reference
                                .get()
                                .ok_or_else(|| anyhow!("dereference of a dangling reference"))?,
                            value => value.clone(),
                        };
                    }
                    Op::SetDeref { target, val } => {
                        let PValue::Ref(reference) = &stack[base + *target as usize] else {
                            bail!("assignment through a non-reference value");
                        };
                        if !reference.set(stack[base + *val as usize].clone()) {
                            bail!("assignment through a dangling reference");
                        }
                    }
                    Op::GetField {
                        dst,
                        base: b,
                        member,
                    } => {
                        let v = self.get_field(
                            &stack[base + *b as usize],
                            &cur.members[*member as usize],
                        )?;
                        stack[base + *dst as usize] = v;
                    }
                    Op::SetField {
                        base: b,
                        member,
                        val,
                    } => {
                        self.set_field(
                            &stack[base + *b as usize],
                            &cur.members[*member as usize],
                            stack[base + *val as usize].clone(),
                        )?;
                    }
                    Op::Try { dst, src } => {
                        match pops::eval_try(stack[base + *src as usize].clone())? {
                            Ok(v) => stack[base + *dst as usize] = v,
                            Err(early) => ret!(early),
                        }
                    }
                    Op::Cast { dst, src, ty } => {
                        let v = eval_cast(
                            &cur.casts[*ty as usize],
                            stack[base + *src as usize].clone(),
                        )?;
                        stack[base + *dst as usize] = v;
                    }
                    Op::Coerce { dst, src, ty } => {
                        let v = self.coerce_value(
                            stack[base + *src as usize].clone(),
                            &cur.coerces[*ty as usize],
                        );
                        stack[base + *dst as usize] = v;
                    }
                    Op::TestBind { val, pat, dst } => {
                        let info = &cur.pats[*pat as usize];
                        let value = stack[base + *val as usize].clone();
                        let binds = &info.binds;
                        let mut writes: Vec<(u16, PValue)> = Vec::new();
                        let matched = {
                            let mut define = |name: &str, v: PValue| {
                                if let Some((_, reg)) = binds.iter().find(|(n, _)| n == name) {
                                    writes.push((*reg, v));
                                }
                            };
                            try_bind(&info.pat, &value, &mut define)
                        };
                        for (reg, v) in writes {
                            stack[base + reg as usize] = v;
                        }
                        stack[base + *dst as usize] = PValue::Bool(matched);
                    }
                    Op::Fmt { dst, spec } => {
                        let text = self.render_fmt(&cur, *spec, &stack[base..])?;
                        stack[base + *dst as usize] = PValue::str(text);
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
                                stack[base + *dst as usize] = PValue::err(PValue::str(text));
                            }
                            MacroKind::Bail => ret!(PValue::err(PValue::str(text))),
                        }
                        if !matches!(kind, MacroKind::Anyhow) {
                            stack[base + *dst as usize] = PValue::Unit;
                        }
                    }
                    Op::Dbg {
                        dst,
                        base: wbase,
                        argc,
                    } => {
                        let (wbase, argc) = (*wbase as usize, *argc as usize);
                        let mut last = PValue::Unit;
                        for i in 0..argc {
                            last = stack[base + wbase + i].clone();
                            eprintln!("[dbg] {}", last.debug());
                        }
                        stack[base + *dst as usize] = last;
                    }
                    Op::Spawn { dst, child } => {
                        let clo = Self::make_closure(
                            &cur,
                            *child,
                            stack,
                            base,
                            &cur_clo,
                            entry_upvalues,
                            &mut local_cells,
                        );
                        let interp = self.clone();
                        let handle = self.rt.spawn_blocking(move || {
                            interp
                                .run_chunk(&clo.chunk, &[], &clo.captured)
                                .unwrap_or_else(|e| PValue::err(PValue::str(e.to_string())))
                        });
                        stack[base + *dst as usize] = PNative::Task(handle).wrap();
                    }
                    Op::Await { dst, src } => {
                        let v = take(&mut stack[base + *src as usize]);
                        stack[base + *dst as usize] = self.await_value(v)?;
                    }
                }
                ip += 1;
            }
        })();
        result.map_err(|e| {
            let trace = std::iter::once(frame_line(&cur, ip)).chain(
                frames
                    .iter()
                    .rev()
                    .map(|f| frame_line(&f.chunk, f.ip.saturating_sub(1))),
            );
            trace_error(e, trace)
        })
    }

    fn make_closure(
        cur: &Arc<PChunk>,
        child: u16,
        stack: &[PValue],
        base: usize,
        cur_clo: &Option<Arc<PClosure>>,
        entry_upvalues: &[PUpvalue],
        local_cells: &mut HashMap<usize, Arc<Mutex<PValue>>>,
    ) -> Arc<PClosure> {
        let child_chunk = cur.children[child as usize].clone();
        let caps = &cur.child_caps[child as usize];
        let upvals: &[PUpvalue] = match cur_clo {
            Some(c) => &c.captured,
            None => entry_upvalues,
        };
        let captured: Vec<PUpvalue> = caps
            .iter()
            .map(|c| match c {
                CapSource::Local(reg) => PUpvalue::Value(stack[base + *reg as usize].clone()),
                CapSource::Upvalue(idx) | CapSource::MutableUpvalue(idx) => {
                    upvals[*idx as usize].clone()
                }
                CapSource::MutableLocal(reg) => {
                    let slot = base + *reg as usize;
                    let value = stack[slot].clone();
                    let cell = local_cells
                        .entry(slot)
                        .or_insert_with(|| Arc::new(Mutex::new(value)))
                        .clone();
                    PUpvalue::Mutable(cell)
                }
            })
            .collect();
        Arc::new(PClosure {
            chunk: child_chunk,
            captured,
        })
    }

    /// Invoke a closure value, for the higher order bridge methods.
    pub(super) fn call_closure(self: &Arc<Self>, f: &PValue, args: &[PValue]) -> Result<PValue> {
        let PValue::Closure(clo) = f else {
            bail!("expected a closure, got {}", f.type_name());
        };
        let chunk = clo.chunk.clone();
        self.run_chunk(&chunk, args, &clo.captured)
    }

    /// Drive an awaited value to its result. A JoinHandle joins, a future is
    /// run to completion, anything else is already a value.
    fn await_value(&self, v: PValue) -> Result<PValue> {
        let PValue::Native(n) = v else { return Ok(v) };
        let taken = replace(&mut *n.lock(), PNative::Taken);
        match taken {
            // Awaiting a JoinHandle yields `Result<T, JoinError>` in real Rust,
            // so it wraps. A script that passes `rust check` writes `.await?` or
            // `.await.unwrap()`, and both need the Ok layer to be here.
            PNative::Task(h) => Ok(match self.rt.block_on(h) {
                Ok(v) => PValue::ok(v),
                Err(e) => PValue::err(PValue::str(e.to_string())),
            }),
            // Awaiting a plain future yields its output directly, no wrapper.
            PNative::Future(f) => Ok(self.rt.block_on(f)),
            PNative::Taken => bail!("this value was already awaited"),
            // Everything else is a live resource, not an awaitable. Put it back
            // so the handle stays usable after the bad await is reported.
            other => {
                let name = other.type_name();
                *n.lock() = other;
                bail!("cannot await a {name}")
            }
        }
    }

    pub(super) fn user_function(&self, name: &str) -> Option<Arc<PChunk>> {
        self.fn_index
            .get(name)
            .map(|&i| self.functions[i as usize].clone())
    }

    pub(super) fn user_method(&self, ty: &str, name: &str) -> Option<Arc<PChunk>> {
        self.methods
            .get(&(ty.to_string(), name.to_string()))
            .cloned()
    }

    /// Value of a module const or static, evaluated on first read and cached.
    fn global(self: &Arc<Self>, idx: usize) -> Result<PValue> {
        {
            match &*self.globals[idx].lock() {
                PGlobalSlot::Ready(v) => return Ok(v.clone()),
                PGlobalSlot::Busy => {
                    bail!("constant initializers depend on each other in a cycle")
                }
                PGlobalSlot::Todo(_) => {}
            }
        }
        let chunk = {
            let mut slot = self.globals[idx].lock();
            match replace(&mut *slot, PGlobalSlot::Busy) {
                PGlobalSlot::Todo(c) => c,
                other => {
                    *slot = other;
                    bail!("constant initializers depend on each other in a cycle");
                }
            }
        };
        let v = self.run_chunk(&chunk, &[], &[])?;
        *self.globals[idx].lock() = PGlobalSlot::Ready(v.clone());
        Ok(v)
    }
}

fn take_range(stack: &mut [PValue], s: usize, count: usize) -> Vec<PValue> {
    (0..count).map(|i| take(&mut stack[s + i])).collect()
}

/// Apply an `as` cast to a value, with the same width semantics as
/// `eval_cast` in eval.rs.
fn eval_cast(target: &CastIr, v: PValue) -> Result<PValue> {
    let width = match target {
        CastIr::F64 => {
            return Ok(PValue::Float(match v {
                PValue::Int(i) => i as f64,
                PValue::IntW(..) => v.int_parts().unwrap().0 as f64,
                PValue::Float(f) => f,
                PValue::F32(f) => f64::from(f),
                other => bail!("cannot cast {} to float", other.type_name()),
            }));
        }
        CastIr::F32 => {
            return Ok(PValue::F32(match v {
                PValue::Int(i) => i as f32,
                PValue::IntW(..) => v.int_parts().unwrap().0 as f32,
                PValue::Float(f) => f as f32,
                PValue::F32(f) => f,
                other => bail!("cannot cast {} to float", other.type_name()),
            }));
        }
        CastIr::Char => {
            return Ok(match v {
                PValue::Int(i) => PValue::Char(
                    char::from_u32(i as u32)
                        .ok_or_else(|| anyhow::anyhow!("invalid char code {i}"))?,
                ),
                PValue::Char(c) => PValue::Char(c),
                other => bail!("cannot cast {} to char", other.type_name()),
            });
        }
        CastIr::Unsupported(name) => bail!("unsupported cast target: {name}"),
        CastIr::Int(width) => *width,
    };
    let value = match v {
        PValue::Int(i) => truncate(i128::from(i), width),
        PValue::IntW(..) => truncate(v.int_parts().unwrap().0, width),
        PValue::Float(f) => float_to_int(f, width),
        PValue::F32(f) => float_to_int(f64::from(f), width),
        PValue::Char(c) => truncate(i128::from(c as u32), width),
        PValue::Bool(b) => i128::from(b),
        other => bail!("cannot cast {} to integer", other.type_name()),
    };
    Ok(PValue::int_of_width(value, width))
}

/// A field access on a struct or tuple, the parallel twin of `get_field`.
impl PInterp {
    pub(super) fn get_field(&self, recv: &PValue, member: &PMember) -> Result<PValue> {
        match (recv, member) {
            (PValue::Struct(s), PMember::Named(n)) => {
                s.get(n).ok_or_else(|| anyhow::anyhow!("no field `{n}`"))
            }
            (PValue::Tuple(t), PMember::Indexed(i)) => t
                .lock()
                .get(*i)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no tuple index {i}")),
            (PValue::Struct(s), PMember::Indexed(i)) => s
                .values
                .lock()
                .get(*i)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no field {i}")),
            _ => bail!("cannot read a field of {}", recv.type_name()),
        }
    }

    pub(super) fn set_field(&self, recv: &PValue, member: &PMember, v: PValue) -> Result<()> {
        match (recv, member) {
            (PValue::Struct(s), PMember::Named(n)) => {
                if !s.set(n, v) {
                    bail!("no field `{n}`");
                }
            }
            (PValue::Tuple(t), PMember::Indexed(i)) => {
                let mut t = t.lock();
                if *i >= t.len() {
                    bail!("no tuple index {i}");
                }
                t[*i] = v;
            }
            _ => bail!("cannot set a field of {}", recv.type_name()),
        }
        Ok(())
    }
}
