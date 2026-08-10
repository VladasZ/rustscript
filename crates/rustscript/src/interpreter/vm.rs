//! The parallel register machine. It mirrors `vm.rs` but runs `Chunk` over
//! `Value`, so a task can run on any worker thread. The hot inline fast paths
//! of the fast VM are dropped here for clarity; the interpreter optimizes
//! for correctness and real concurrency, not single thread microspeed.

use num_traits::AsPrimitive;
use std::collections::HashMap;
use std::mem::{replace, take};
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use parking_lot::Mutex;
use tokio::runtime::Handle;

use super::bytecode::{BuiltinId, CapSource, MacroKind, MethodName, Op};
use super::bytecode::{Chunk, Member};
use super::native::Native;
use super::numeric::{float_to_int, truncate};
use super::ops::{
    self, apply_bin, apply_bin_imm, apply_un, cmp_test, cmp_test_imm, int_of, try_bind,
};
use super::typeir::{CastIr, TypeIr};
use super::value::{ClosureData, StructShape, Upvalue, Value};
use super::vm_support::trace_error;

const MAX_CALL_DEPTH: usize = 100_000;

fn swap_option<T>(current: &mut Option<T>, next: Option<T>) -> Option<T> {
    match next {
        Some(value) => current.replace(value),
        None => current.take(),
    }
}

/// One backtrace entry: the function, its file, and the line of the op at
/// `ip`.
fn frame_line(chunk: &Chunk, ip: usize) -> (String, String, u32) {
    let line = chunk.lines.get(ip).copied().unwrap_or(0);
    (chunk.name.clone(), chunk.file.to_string(), line)
}

/// A module level const or static: converted once, evaluated on first read.
/// Each slot has its own lock so tasks on different threads can read globals.
pub enum GlobalSlot {
    Todo(Arc<Chunk>),
    Busy,
    Ready(Value),
}

/// A user enum's variants, precomputed at load so runtime dispatch never
/// touches the syn AST, which is not `Send`.
pub struct EnumDef {
    pub name: Arc<str>,
    /// Variant name and whether it is a unit variant.
    pub variants: Vec<(Arc<str>, bool)>,
}

/// The compiled program plus the runtime handle, shared across worker threads.
pub struct Vm {
    pub functions: Vec<Arc<Chunk>>,
    pub fn_index: HashMap<String, u32>,
    pub methods: HashMap<(String, String), Arc<Chunk>>,
    pub globals: Vec<Mutex<GlobalSlot>>,
    /// User struct layouts precomputed at load, for coercion and typed json.
    pub structs: super::json_bridge::Structs,
    /// Root module imports, used by the bridge dispatch to expand aliases.
    pub uses: HashMap<String, Vec<String>>,
    /// User enums with their variants, for dynamic variant construction.
    pub enums: Vec<EnumDef>,
    /// Canonical names of unit structs, for `struct Marker;` used as a value.
    pub unit_structs: Vec<Arc<str>>,
    /// Canonical names of every user struct, for tuple struct calls.
    pub struct_names: std::collections::HashSet<String>,
    pub rt: Handle,
}

impl Vm {
    /// Expand the first path segment through the `use` table.
    pub(super) fn canonical(&self, segs: &[String]) -> Vec<String> {
        if let Some(full) = self.uses.get(&segs[0]) {
            let mut out = full.clone();
            out.extend_from_slice(&segs[1..]);
            out
        } else {
            segs.to_vec()
        }
    }

    /// A unit variant of a user enum, matched by canonical or bare enum name.
    pub(super) fn unit_variant(&self, enum_name: Option<&str>, variant: &str) -> Option<Value> {
        for def in &self.enums {
            if let Some(want) = enum_name
                && want != &*def.name
                && want != super::resolver::bare(&def.name)
            {
                continue;
            }
            if def
                .variants
                .iter()
                .any(|(name, unit)| &**name == variant && *unit)
            {
                return Some(Value::Enum {
                    enum_name: def.name.clone(),
                    variant: Arc::from(variant),
                    data: Arc::from(Vec::new()),
                });
            }
        }
        None
    }

    /// A tuple variant of a user enum built from call arguments.
    pub(super) fn make_tuple_variant(
        &self,
        enum_name: Option<&str>,
        variant: &str,
        args: &[Value],
    ) -> Option<Value> {
        for def in &self.enums {
            if let Some(want) = enum_name
                && want != &*def.name
                && want != super::resolver::bare(&def.name)
            {
                continue;
            }
            if def.variants.iter().any(|(name, _)| &**name == variant) {
                return Some(Value::Enum {
                    enum_name: def.name.clone(),
                    variant: Arc::from(variant),
                    data: args.iter().cloned().collect(),
                });
            }
        }
        None
    }

    pub(super) fn make_tuple_struct(name: &str, args: Vec<Value>) -> Value {
        let fields = (0..args.len()).map(|i| i.to_string().into()).collect();
        Value::structure(super::value::StructShape::new(name, fields), args)
    }

    /// If a Ctrl-C arrived, run the script's registered handler closure.
    pub(super) fn run_pending_ctrlc(self: &Arc<Self>) -> Result<()> {
        if let Some(handler) = super::pending_ctrlc_handler()
            && let Value::Closure(clo) = handler
        {
            self.call_closure_data(&clo, &[])?;
        }
        Ok(())
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
}

/// A binding of a generic parameter name to the lowered concrete type a
/// caller passed by turbofish, the parallel twin of `TypeEnv` in vm.rs.
pub(super) type TypeEnv = Arc<[(Arc<str>, TypeIr)]>;

fn empty_type_env() -> TypeEnv {
    Arc::from(Vec::new())
}

struct Frame {
    chunk: Arc<Chunk>,
    closure: Option<Arc<ClosureData>>,
    ip: usize,
    base: usize,
    dst: u16,
    /// The caller's arg window, so the callee's final parameter values can be
    /// handed back on return for `&mut` argument writebacks.
    abase: u16,
    argc: u16,
    type_env: TypeEnv,
}

impl Vm {
    pub fn run_chunk(
        self: &Arc<Self>,
        chunk: &Arc<Chunk>,
        args: &[Value],
        upvalues: &[Upvalue],
    ) -> Result<Value> {
        if args.len() != chunk.num_params {
            bail!(
                "`{}` expects {} args but got {}",
                chunk.name,
                chunk.num_params,
                args.len()
            );
        }
        let mut stack = vec![Value::Unit; chunk.num_regs.max(chunk.num_params)];
        for (i, a) in args.iter().enumerate() {
            stack[i] = a.clone();
        }
        self.exec(chunk, &mut stack, upvalues)
    }

    fn exec(
        self: &Arc<Self>,
        entry: &Arc<Chunk>,
        stack: &mut Vec<Value>,
        entry_upvalues: &[Upvalue],
    ) -> Result<Value> {
        let mut frames: Vec<Frame> = Vec::new();
        let mut local_cells: HashMap<usize, Arc<Mutex<Value>>> = HashMap::new();
        let mut cur = entry.clone();
        let mut cur_clo: Option<Arc<ClosureData>> = None;
        let mut cur_tenv: TypeEnv = empty_type_env();
        let mut base = 0usize;
        let mut ip = 0usize;

        // The dispatch runs inside one immediately called closure so an error
        // can be annotated with the script call chain still held in `frames`
        // and the failing op still addressed by `cur` and `ip`. The closure
        // runs exactly once, so the hot loop itself is unchanged.
        let result = (|| -> Result<Value> {
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
                    call!($chunk, $clo, $dst, $abase, $argc, empty_type_env())
                };
                ($chunk:expr, $clo:expr, $dst:expr, $abase:expr, $argc:expr, $tenv:expr) => {{
                    let callee: Arc<Chunk> = $chunk;
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
                    for slot in &mut stack[nbase + $argc..need] {
                        *slot = Value::Unit;
                    }
                    frames.push(Frame {
                        chunk: replace(&mut cur, callee),
                        closure: swap_option(&mut cur_clo, $clo),
                        ip: ip + 1,
                        base,
                        dst: $dst,
                        abase: u16::try_from($abase).expect("register index fits u16"),
                        argc: u16::try_from($argc).expect("argument count fits u16"),
                        type_env: replace(&mut cur_tenv, $tenv),
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
                        stack[base + *dst as usize] = Value::from_const(&cur.consts[*k as usize]);
                    }
                    Op::LoadInt { dst, v } => stack[base + *dst as usize] = Value::Int(*v),
                    Op::LoadIntW { dst, v, w } => {
                        stack[base + *dst as usize] = Value::IntW(*v, *w);
                    }
                    Op::LoadBool { dst, v } => stack[base + *dst as usize] = Value::Bool(*v),
                    Op::LoadUnit { dst } => stack[base + *dst as usize] = Value::Unit,
                    Op::LoadGlobal { dst, idx } => {
                        let v = self.global(*idx as usize)?;
                        stack[base + *dst as usize] = v;
                    }
                    Op::LoadUpvalue { dst, idx } => {
                        let upvals: &[Upvalue] = match &cur_clo {
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
                        let upvalues: &[Upvalue] = match &cur_clo {
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
                        let to = *to as usize;
                        // A backward jump closes a loop iteration, the moment
                        // to run a pending Ctrl-C handler.
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
                        let tenv: TypeEnv = if *targ == u32::MAX {
                            empty_type_env()
                        } else {
                            let targs = &cur.call_type_args[*targ as usize];
                            callee
                                .generics
                                .iter()
                                .zip(targs.iter())
                                .map(|(name, ty)| (name.clone(), ty.clone()))
                                .collect()
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
                            Value::Closure(clo) => clo.clone(),
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
                        if let Some(v) = Self::internal_path(segs, &stack[base..], abase, argc)? {
                            stack[base + *dst as usize] = v;
                            ip += 1;
                            continue;
                        }
                        let call_args = take_range(stack, base + abase, argc);
                        // Typed json parses straight into the target structs,
                        // no generic tree and no coercion pass afterwards.
                        if let Some(ty) = coerce {
                            let canon = self.canonical(segs);
                            if canon.len() >= 2
                                && canon[canon.len() - 2] == "serde_json"
                                && canon[canon.len() - 1] == "from_str"
                            {
                                let v = self.typed_from_str(&call_args, ty, &cur_tenv)?;
                                stack[base + *dst as usize] = v;
                                ip += 1;
                                continue;
                            }
                        }
                        let mut v = self.dispatch_call(segs, call_args)?;
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
                        let (dst, recv) = (*dst, *recv as usize);
                        let (abase, argc) = (*abase as usize, *argc as usize);
                        let name = &cur.names[*name as usize];
                        let s = base + abase;
                        // A string is an immutable `Arc<str>`, so a push has to
                        // rewrite the receiver register itself. The normal path
                        // hands the method a clone and the change would be lost.
                        // `clone_from` replaces the receiver outright, so it has
                        // to write the register rather than a copy of it.
                        if name.id == BuiltinId::CloneFrom {
                            let src = stack[s..s + argc].first().cloned().unwrap_or(Value::Unit);
                            stack[base + recv] = src;
                            if dst != u16::MAX {
                                stack[base + dst as usize] = Value::Unit;
                            }
                            ip += 1;
                            continue;
                        }
                        if matches!(name.id, BuiltinId::Push | BuiltinId::PushStr)
                            && let Value::Str(text) = &stack[base + recv]
                        {
                            let mut out = text.to_string();
                            match (&name.id, stack[s..s + argc].first()) {
                                (BuiltinId::Push, Some(Value::Char(c))) => out.push(*c),
                                (BuiltinId::PushStr, Some(arg)) => out.push_str(&arg.display()),
                                _ => {}
                            }
                            stack[base + recv] = Value::str(out);
                            if dst != u16::MAX {
                                stack[base + dst as usize] = Value::Unit;
                            }
                            ip += 1;
                            continue;
                        }
                        // Option and Result accessors dominate counting loops,
                        // so their success paths run right here, skipping the
                        // whole dispatch chain. Failure paths fall through and
                        // get their errors from the slow path. Skipped when the
                        // script defines methods, which could shadow these.
                        if self.methods.is_empty()
                            && matches!(
                                name.id,
                                BuiltinId::Copied | BuiltinId::Unwrap | BuiltinId::UnwrapOr
                            )
                        {
                            // 0 none, 1 clone receiver, 2 clone payload, 3 default
                            let choice = match &stack[base + recv] {
                                Value::Enum {
                                    enum_name, variant, ..
                                } => {
                                    if matches!(name.id, BuiltinId::Copied) {
                                        i32::from(&**enum_name == "Option")
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
                                if dst != u16::MAX {
                                    stack[base + dst as usize] = v;
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
                            if dst != u16::MAX {
                                let v = Value::Str(v.clone());
                                stack[base + dst as usize] = v;
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
                        ) && matches!(
                            stack[base + recv],
                            Value::Map(_, super::value::MapKind::Map)
                        ) && argc >= 1
                            && base + recv < s
                        {
                            let (lo, hi) = stack.split_at_mut(s);
                            let Value::Map(m, _) = &lo[base + recv] else {
                                unreachable!()
                            };
                            let v = if name.id == BuiltinId::Insert {
                                let k = take(&mut hi[0]).into_key();
                                let Some(k) = k else { bail!("invalid map key") };
                                let val = if argc > 1 {
                                    take(&mut hi[1])
                                } else {
                                    Value::Unit
                                };
                                let old = m.lock().insert(k, val);
                                if dst == u16::MAX {
                                    Value::Unit
                                } else {
                                    match old {
                                        Some(old) => Value::some(old),
                                        None => Value::none(),
                                    }
                                }
                            } else {
                                let Some(k) = hi[0].as_key() else {
                                    bail!("invalid map key")
                                };
                                if matches!(name.id, BuiltinId::ContainsKey) {
                                    Value::Bool(m.lock().get(&k).is_some())
                                } else {
                                    match m.lock().get(&k).cloned() {
                                        Some(v) => Value::some(v),
                                        None => Value::none(),
                                    }
                                }
                            };
                            if dst != u16::MAX {
                                stack[base + dst as usize] = v;
                            }
                            ip += 1;
                            continue;
                        }
                        // The arg window holds dead temporaries, so methods may
                        // consume or mutate them in place. A `read_line(&mut s)`
                        // buffer lands back in its register this way.
                        let v = if argc == 0 {
                            self.eval_method(&stack[base + recv].clone(), name, &mut [])?
                        } else if base + recv < s {
                            let (lo, hi) = stack.split_at_mut(s);
                            self.eval_method(&lo[base + recv], name, &mut hi[..argc])?
                        } else {
                            let recv_v = stack[base + recv].clone();
                            self.eval_method(&recv_v, name, &mut stack[s..s + argc])?
                        };
                        if dst != u16::MAX {
                            stack[base + dst as usize] = v;
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
                            scalar: None,
                        };
                        let opt = self.eval_method(&recv_v, &get, &mut [key_v])?;
                        let v = match opt {
                            Value::Enum { variant, data, .. } if &*variant == "Some" => {
                                data.first().cloned().unwrap_or(Value::Unit)
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
                        stack[base + *dst as usize] = Value::vec(items);
                    }
                    Op::MakeTuple {
                        dst,
                        base: wbase,
                        count,
                    } => {
                        let items = take_range(stack, base + *wbase as usize, *count as usize);
                        stack[base + *dst as usize] = Value::tuple(items);
                    }
                    Op::MakeArrayRepeat { dst, val, count } => {
                        let n = match &stack[base + *count as usize] {
                            Value::Int(n) => usize::try_from(*n)?,
                            v if v.untag_int().is_some() => {
                                usize::try_from(v.untag_int().unwrap())?
                            }
                            _ => bail!("array repeat length must be an integer"),
                        };
                        let v = stack[base + *val as usize].clone();
                        stack[base + *dst as usize] =
                            Value::vec(std::iter::repeat_n(v, n).collect());
                    }
                    Op::MakeRange {
                        dst,
                        start,
                        end,
                        inclusive,
                    } => {
                        let s = int_of(&stack[base + *start as usize])?;
                        let e = int_of(&stack[base + *end as usize])?;
                        stack[base + *dst as usize] = Value::Range {
                            start: s,
                            end: e,
                            inclusive: *inclusive,
                        };
                    }
                    Op::IterInit { dst, src } => {
                        let src_v = stack[base + *src as usize].clone();
                        let it = Self::iterator_value(src_v)?;
                        stack[base + *dst as usize] = it;
                    }
                    Op::ForNext { iter, idx, val, to } => {
                        let i = match &stack[base + *idx as usize] {
                            Value::Int(i) => *i,
                            _ => unreachable!("for index is an integer"),
                        };
                        let item = match stack[base + *iter as usize].clone() {
                            Value::Native(iterator) => self.iterator_next(&iterator)?,
                            other => bail!("{} is not an iterator", other.type_name()),
                        };
                        if let Some(v) = item {
                            stack[base + *val as usize] = v;
                            self.run_pending_ctrlc()?;
                            stack[base + *idx as usize] = Value::Int(i + 1);
                        } else {
                            ip = *to as usize;
                            continue;
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
                        let mut values: Vec<Value> = (0..written)
                            .map(|k| take(&mut stack[base + wbase + k]))
                            .collect();
                        let v = if lit.has_rest {
                            let rest = stack[base + wbase + written].clone();
                            let mut fields = lit.shape.fields.clone();
                            let mut renames = lit.shape.renames.clone();
                            if let Value::Struct(r) = rest {
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
                            let shape = Arc::new(StructShape {
                                name: lit.shape.name.clone(),
                                fields,
                                renames,
                            });
                            Value::structure(shape, values)
                        } else {
                            Value::structure(lit.shape.clone(), values)
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
                        stack[base + *dst as usize] = Value::Enum {
                            enum_name: variant.enum_name.clone(),
                            variant: variant.variant.clone(),
                            data,
                        };
                    }
                    Op::LoadEnum { dst, info } => {
                        let variant = &cur.enum_variants[*info as usize];
                        stack[base + *dst as usize] = Value::Enum {
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
                            cur_clo.as_ref(),
                            entry_upvalues,
                            &mut local_cells,
                        );
                        stack[base + *dst as usize] = Value::Closure(clo);
                    }
                    Op::Index { dst, base: b, key } => {
                        let v =
                            ops::index(&stack[base + *b as usize], &stack[base + *key as usize])?;
                        stack[base + *dst as usize] = v;
                    }
                    Op::SetIndex { base: b, key, val } => {
                        ops::set_index(
                            &stack[base + *b as usize],
                            &stack[base + *key as usize],
                            stack[base + *val as usize].clone(),
                        )?;
                    }
                    Op::Deref { dst, src } => {
                        stack[base + *dst as usize] = match &stack[base + *src as usize] {
                            Value::Ref(reference) => reference
                                .get()
                                .ok_or_else(|| anyhow!("dereference of a dangling reference"))?,
                            value => value.clone(),
                        };
                    }
                    Op::SetDeref { target, val } => {
                        let Value::Ref(reference) = &stack[base + *target as usize] else {
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
                        let v = Self::get_field(
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
                        Self::set_field(
                            &stack[base + *b as usize],
                            &cur.members[*member as usize],
                            stack[base + *val as usize].clone(),
                        )?;
                    }
                    Op::Try { dst, src } => {
                        match ops::eval_try(stack[base + *src as usize].clone()) {
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
                            stack[base + reg as usize] = v;
                        }
                        stack[base + *dst as usize] = Value::Bool(matched);
                    }
                    Op::Fmt { dst, spec } => {
                        let text = Self::render_fmt(&cur, *spec, &stack[base..])?;
                        stack[base + *dst as usize] = Value::str(text);
                    }
                    Op::MacroCall { kind, dst, spec } => {
                        let text = Self::render_fmt(&cur, *spec, &stack[base..])?;
                        match kind {
                            MacroKind::Println => println!("{text}"),
                            MacroKind::Print => print!("{text}"),
                            MacroKind::Eprintln => eprintln!("{text}"),
                            MacroKind::Eprint => eprint!("{text}"),
                            MacroKind::Panic => bail!("panicked: {text}"),
                            MacroKind::Anyhow => {
                                stack[base + *dst as usize] = Value::err(Value::str(text));
                            }
                            MacroKind::Bail => ret!(Value::err(Value::str(text))),
                        }
                        if !matches!(kind, MacroKind::Anyhow) {
                            stack[base + *dst as usize] = Value::Unit;
                        }
                    }
                    Op::Dbg {
                        dst,
                        base: wbase,
                        argc,
                    } => {
                        let (wbase, argc) = (*wbase as usize, *argc as usize);
                        let mut last = Value::Unit;
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
                            cur_clo.as_ref(),
                            entry_upvalues,
                            &mut local_cells,
                        );
                        let interp = self.clone();
                        let handle = self.rt.spawn_blocking(move || {
                            interp
                                .run_chunk(&clo.chunk, &[], &clo.captured)
                                .unwrap_or_else(|e| Value::err(Value::str(e.to_string())))
                        });
                        stack[base + *dst as usize] = Native::Task(handle).wrap();
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
        cur: &Arc<Chunk>,
        child: u16,
        stack: &[Value],
        base: usize,
        cur_clo: Option<&Arc<ClosureData>>,
        entry_upvalues: &[Upvalue],
        local_cells: &mut HashMap<usize, Arc<Mutex<Value>>>,
    ) -> Arc<ClosureData> {
        let child_chunk = cur.children[child as usize].clone();
        let caps = &cur.child_caps[child as usize];
        let upvals: &[Upvalue] = match cur_clo {
            Some(c) => &c.captured,
            None => entry_upvalues,
        };
        let captured: Vec<Upvalue> = caps
            .iter()
            .map(|c| match c {
                CapSource::Local(reg) => Upvalue::Value(stack[base + *reg as usize].clone()),
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
                    Upvalue::Mutable(cell)
                }
            })
            .collect();
        Arc::new(ClosureData {
            chunk: child_chunk,
            captured,
        })
    }

    /// Drive an awaited value to its result. A `JoinHandle` joins, a future is
    /// run to completion, anything else is already a value.
    fn await_value(&self, v: Value) -> Result<Value> {
        let Value::Native(n) = v else { return Ok(v) };
        let taken = replace(&mut *n.lock(), Native::Taken);
        match taken {
            // Awaiting a JoinHandle yields `Result<T, JoinError>` in real Rust,
            // so it wraps. A script that passes `rust check` writes `.await?` or
            // `.await.unwrap()`, and both need the Ok layer to be here.
            Native::Task(h) => Ok(match self.rt.block_on(h) {
                Ok(v) => Value::ok(v),
                Err(e) => Value::err(Value::str(e.to_string())),
            }),
            // Awaiting a plain future yields its output directly, no wrapper.
            Native::Future(f) => Ok(self.rt.block_on(f)),
            Native::Taken => bail!("this value was already awaited"),
            // Everything else is a live resource, not an awaitable. Put it back
            // so the handle stays usable after the bad await is reported.
            other => {
                let name = other.type_name();
                *n.lock() = other;
                bail!("cannot await a {name}")
            }
        }
    }

    pub(super) fn user_function(&self, name: &str) -> Option<Arc<Chunk>> {
        self.fn_index
            .get(name)
            .map(|&i| self.functions[i as usize].clone())
    }

    pub(super) fn user_method(&self, ty: &str, name: &str) -> Option<Arc<Chunk>> {
        self.methods
            .get(&(ty.to_string(), name.to_string()))
            .cloned()
    }

    /// Value of a module const or static, evaluated on first read and cached.
    fn global(self: &Arc<Self>, idx: usize) -> Result<Value> {
        {
            match &*self.globals[idx].lock() {
                GlobalSlot::Ready(v) => return Ok(v.clone()),
                GlobalSlot::Busy => {
                    bail!("constant initializers depend on each other in a cycle")
                }
                GlobalSlot::Todo(_) => {}
            }
        }
        let chunk = {
            let mut slot = self.globals[idx].lock();
            match replace(&mut *slot, GlobalSlot::Busy) {
                GlobalSlot::Todo(c) => c,
                other => {
                    *slot = other;
                    bail!("constant initializers depend on each other in a cycle");
                }
            }
        };
        let v = self.run_chunk(&chunk, &[], &[])?;
        *self.globals[idx].lock() = GlobalSlot::Ready(v.clone());
        Ok(v)
    }
}

fn take_range(stack: &mut [Value], s: usize, count: usize) -> Vec<Value> {
    (0..count).map(|i| take(&mut stack[s + i])).collect()
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
                        .ok_or_else(|| anyhow::anyhow!("invalid char code {i}"))?,
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

/// A field access on a struct or tuple, the parallel twin of `get_field`.
impl Vm {
    pub(super) fn get_field(recv: &Value, member: &Member) -> Result<Value> {
        match (recv, member) {
            (Value::Struct(s), Member::Named(n)) => {
                s.get(n).ok_or_else(|| anyhow::anyhow!("no field `{n}`"))
            }
            (Value::Tuple(t), Member::Indexed(i)) => t
                .lock()
                .get(*i)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no tuple index {i}")),
            (Value::Struct(s), Member::Indexed(i)) => s
                .values
                .lock()
                .get(*i)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no field {i}")),
            _ => bail!("cannot read a field of {}", recv.type_name()),
        }
    }

    pub(super) fn set_field(recv: &Value, member: &Member, v: Value) -> Result<()> {
        match (recv, member) {
            (Value::Struct(s), Member::Named(n)) => {
                if !s.set(n, v) {
                    bail!("no field `{n}`");
                }
            }
            (Value::Tuple(t), Member::Indexed(i)) => {
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
