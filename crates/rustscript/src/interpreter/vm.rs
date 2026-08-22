//! The frame loop. `exec` owns the call frames and applies the `Flow` each op returns. The op
//! bodies are in `vm_step`.

use std::collections::HashMap;
use std::mem::{replace, take};
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use parking_lot::Mutex;
use tokio::runtime::Handle;

use super::bytecode::{Chunk, Member, path_call_chunk};
use super::enum_def::EnumDef;
use super::impls::ImplTable;
use super::native::Native;
use super::typeir::TypeIr;
use super::value::{ClosureData, StructShape, Upvalue, Value};
use super::vm_step::{Flow, StepCtx, step};
use super::vm_support::trace_error;

pub(super) const MAX_CALL_DEPTH: usize = 100_000;

/// Deeper nesting gives buffers back to the allocator, so a burst of recursion doesn't pin its
/// memory forever.
const MAX_POOLED_STACKS: usize = 32;

thread_local! {
    /// 1 popped per live call on this thread
    static STACK_POOL: std::cell::RefCell<Vec<Vec<Value>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn swap_option<T>(current: &mut Option<T>, next: Option<T>) -> Option<T> {
    match next {
        Some(value) => current.replace(value),
        None => current.take(),
    }
}

fn frame_line(chunk: &Chunk, ip: usize) -> (String, String, u32) {
    let line = chunk.lines.get(ip).copied().unwrap_or(0);
    (chunk.name.clone(), chunk.file.to_string(), line)
}

/// Each slot has its own lock so tasks on different threads can read globals.
pub enum GlobalSlot {
    Todo(Arc<Chunk>),
    Busy,
    Ready(Value),
}

pub struct Vm {
    pub functions: Vec<Arc<Chunk>>,
    pub fn_index: HashMap<String, u32>,
    pub impls: Arc<ImplTable>,
    pub globals: Vec<Mutex<GlobalSlot>>,
    /// for coercion and typed json
    pub structs: super::json_bridge::Structs,
    /// for dynamic variant construction
    pub enums: Vec<Arc<EnumDef>>,
    /// for `struct Marker;` used as a value
    pub unit_structs: Vec<Arc<str>>,
    /// for tuple struct calls
    pub struct_names: std::collections::HashSet<String>,
    pub rt: Handle,
}

impl Vm {
    /// matched by canonical or bare enum name
    pub(super) fn unit_variant(&self, enum_name: Option<&str>, variant: &str) -> Option<Value> {
        for def in &self.enums {
            if let Some(want) = enum_name
                && want != &*def.name
                && want != super::resolver::bare(&def.name)
            {
                continue;
            }
            if let Some(index) = def.variant_index(variant)
                && def.is_unit(index)
            {
                return Some(Value::enum_of(def, index, Vec::new()));
            }
        }
        None
    }

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
            if let Some(index) = def.variant_index(variant) {
                return Some(Value::enum_of(def, index, args.to_vec()));
            }
        }
        None
    }

    pub(super) fn make_tuple_struct(&self, name: &str, args: Vec<Value>) -> Value {
        let fields = (0..args.len()).map(|i| i.to_string().into()).collect();
        let type_id = self.impls.type_id(name);
        Value::structure(StructShape::typed(name, type_id, fields, Vec::new()), args)
    }

    pub(super) fn run_pending_ctrlc(self: &Arc<Self>) -> Result<()> {
        if let Some(handler) = super::pending_ctrlc_handler()
            && let Value::Closure(clo) = handler
        {
            self.call_closure_data(&clo, &[])?;
        }
        Ok(())
    }
}

pub(super) type TypeEnv = Arc<[(Arc<str>, TypeIr)]>;

pub(super) fn empty_type_env() -> TypeEnv {
    Arc::from(Vec::new())
}

struct Frame {
    chunk: Arc<Chunk>,
    closure: Option<Arc<ClosureData>>,
    ip: usize,
    base: usize,
    dst: u16,
    /// So the final parameter values of the callee can be handed back for `&mut` writebacks.
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
        // A forwarder's arity is a guess. `u8::saturating_add` handed to `fold` takes 2 where the
        // guess was 1.
        let rebuilt;
        let chunk = if chunk.path_forwarder && args.len() != chunk.num_params {
            rebuilt = path_call_chunk(chunk.paths[0].clone(), args.len());
            &rebuilt
        } else {
            chunk
        };
        if args.len() != chunk.num_params {
            bail!(
                "`{}` expects {} args but got {}",
                chunk.name,
                chunk.num_params,
                args.len()
            );
        }
        // A fresh stack per call was the main cost in comparator sorts. Nested calls each pop
        // their own buffer.
        let mut stack = STACK_POOL
            .with(|pool| pool.borrow_mut().pop())
            .unwrap_or_default();
        stack.resize(chunk.num_regs.max(chunk.num_params), Value::Unit);
        for (i, a) in args.iter().enumerate() {
            stack[i] = a.clone();
        }
        let result = self.exec(chunk, &mut stack, upvalues);
        stack.clear();
        STACK_POOL.with(|pool| {
            let mut pool = pool.borrow_mut();
            if pool.len() < MAX_POOLED_STACKS {
                pool.push(stack);
            }
        });
        result
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

        // One immediately called closure, so an error can be annotated with the call chain still in
        // `frames` and the failing op in `cur` and `ip`.
        let result = (|| -> Result<Value> {
            loop {
                let flow = match cur.code.get(ip) {
                    None => Flow::Ret(Value::Unit),
                    Some(op) => step(
                        &mut StepCtx {
                            vm: self,
                            cur: &cur,
                            cur_clo: &cur_clo,
                            cur_tenv: &cur_tenv,
                            entry_upvalues,
                            local_cells: &mut local_cells,
                            stack: &mut *stack,
                            base,
                            ip,
                            depth: frames.len(),
                        },
                        op,
                    )?,
                };
                match flow {
                    Flow::Next => ip += 1,
                    Flow::Jump(to) => ip = to,
                    Flow::Ret(v) => {
                        let Some(f) = frames.pop() else { return Ok(v) };
                        let callee_base = base;
                        let callee_end = callee_base + cur.num_regs;
                        local_cells.retain(|slot, _| *slot < callee_base || *slot >= callee_end);
                        cur = f.chunk;
                        cur_clo = f.closure;
                        cur_tenv = f.type_env;
                        ip = f.ip;
                        // the `&mut` argument writeback picks these up from the caller's arg window
                        base = f.base;
                        for i in 0..f.argc as usize {
                            stack[base + f.abase as usize + i] = take(&mut stack[callee_base + i]);
                        }
                        stack[base + f.dst as usize] = v;
                    }
                    Flow::Call(req) => {
                        if frames.len() >= MAX_CALL_DEPTH {
                            bail!("stack overflow: call depth exceeded {MAX_CALL_DEPTH}");
                        }
                        let nbase = base + cur.num_regs;
                        let need = nbase + req.chunk.num_regs.max(req.chunk.num_params);
                        if stack.len() < need {
                            stack.resize(need, Value::Unit);
                        }
                        for i in 0..req.argc {
                            stack[nbase + i] = take(&mut stack[base + req.abase + i]);
                        }
                        for slot in &mut stack[nbase + req.argc..need] {
                            *slot = Value::Unit;
                        }
                        frames.push(Frame {
                            chunk: replace(&mut cur, req.chunk),
                            closure: swap_option(&mut cur_clo, req.closure),
                            ip: ip + 1,
                            base,
                            dst: req.dst,
                            abase: u16::try_from(req.abase).expect("register index fits u16"),
                            argc: u16::try_from(req.argc).expect("argument count fits u16"),
                            type_env: replace(&mut cur_tenv, req.type_env),
                        });
                        base = nbase;
                        ip = 0;
                    }
                }
            }
        })();
        result.map_err(|e| {
            self.unwind_drops(&cur, base, &frames, stack);
            let trace = std::iter::once(frame_line(&cur, ip)).chain(
                frames
                    .iter()
                    .rev()
                    .map(|f| frame_line(&f.chunk, f.ip.saturating_sub(1))),
            );
            trace_error(e, trace)
        })
    }

    /// Innermost frame first and highest register first, like real unwinding. A panic inside a drop is
    /// reported and the original panic keeps going. Real Rust would abort there.
    fn unwind_drops(
        self: &Arc<Self>,
        cur: &Arc<Chunk>,
        base: usize,
        frames: &[Frame],
        stack: &mut [Value],
    ) {
        let spans = std::iter::once((base, cur.num_regs))
            .chain(frames.iter().rev().map(|f| (f.base, f.chunk.num_regs)));
        for (base, num_regs) in spans {
            for reg in (0..num_regs).rev() {
                let Some(slot) = stack.get_mut(base + reg) else {
                    continue;
                };
                let value = take(slot);
                if let Err(e) = self.run_user_drop(value) {
                    eprintln!("panic in drop during unwinding: {e:#}");
                }
            }
        }
    }

    pub(super) fn await_value(&self, v: Value) -> Result<Value> {
        let Value::Native(n) = v else { return Ok(v) };
        let taken = replace(&mut *n.lock(), Native::Taken);
        match taken {
            // a `JoinHandle` yields `Result<T, JoinError>`, so `.await?` needs the Ok layer
            Native::Task(h) => Ok(match self.rt.block_on(h) {
                Ok(v) => Value::ok(v),
                // both format forms of the real `JoinError`, so `{e:?}` prints what a compiled
                // binary prints
                Err(e) => Value::err(
                    Native::JoinErr {
                        display: e.to_string(),
                        debug: format!("{e:?}"),
                        is_panic: e.is_panic(),
                    }
                    .wrap(),
                ),
            }),
            Native::Future(f) => Ok(self.rt.block_on(f)),
            Native::Taken => bail!("this value was already awaited"),
            // put a non awaitable back so the handle stays usable after the error
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
        self.impls.by_name(ty, name)
    }

    /// Registered under `from<S>`. The plain `from` entry is used when the type has only 1 impl.
    pub(super) fn conversion_impl(&self, target: &str, value: &Value) -> Option<Arc<Chunk>> {
        let source = source_type_name(value);
        let base = source.split(['<', '(']).next().unwrap_or(&source);
        // A value already of the target type needs no conversion. Running a `From` impl here can
        // pick an unrelated variant.
        if base == super::resolver::bare(target) {
            return None;
        }
        self.impls
            .by_name(target, &format!("from<{source}>"))
            .or_else(|| self.impls.by_name(target, &format!("from<{base}>")))
            .or_else(|| self.impls.by_name(target, "from"))
    }

    /// evaluated on first read and cached
    pub(super) fn global(self: &Arc<Self>, idx: usize) -> Result<Value> {
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

impl Vm {
    pub(super) fn get_field(recv: &Value, member: &Member) -> Result<Value> {
        match (recv, member) {
            (Value::Struct(s), Member::Named(n)) => n
                .slot_in(&s.shape)
                .and_then(|i| s.values.lock().get(i).cloned())
                .ok_or_else(|| anyhow!("no field `{n}`")),
            (Value::Tuple(t), Member::Indexed(i)) => t
                .lock()
                .get(*i)
                .cloned()
                .ok_or_else(|| anyhow!("no tuple index {i}")),
            (Value::Struct(s), Member::Indexed(i)) => s
                .values
                .lock()
                .get(*i)
                .cloned()
                .ok_or_else(|| anyhow!("no field {i}")),
            _ => bail!("cannot read a field of {}", recv.type_name()),
        }
    }

    pub(super) fn set_field(recv: &Value, member: &Member, v: Value) -> Result<()> {
        match (recv, member) {
            (Value::Struct(s), Member::Named(n)) => {
                let Some(i) = n.slot_in(&s.shape) else {
                    bail!("no field `{n}`");
                };
                s.values.lock()[i] = v;
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

/// The type name a `From` impl is keyed by for this value.
fn source_type_name(value: &Value) -> String {
    match value {
        Value::Struct(s) => super::resolver::bare(s.name()).to_string(),
        // `Some(x)` names its payload, `None` goes through the bare `Option` key
        Value::Enum { def, data, .. } if def.kind == super::enum_def::EnumKind::Option => {
            let base = super::resolver::bare(&def.name).to_string();
            match data.lock().first() {
                Some(inner) => format!("{base}<{}>", source_type_name(inner)),
                None => base,
            }
        }
        Value::Enum { def, .. } => super::resolver::bare(&def.name).to_string(),
        Value::Tuple(items) => {
            let inner: Vec<String> = items.lock().iter().map(source_type_name).collect();
            format!("({})", inner.join(","))
        }
        Value::Str(_) => "String".to_string(),
        Value::Int(_) => "i64".to_string(),
        Value::IntW(_, w) | Value::Big(_, w) => format!("{w:?}").to_lowercase(),
        Value::Float(_) => "f64".to_string(),
        Value::F32(_) => "f32".to_string(),
        Value::Bool(_) => "bool".to_string(),
        Value::Char(_) => "char".to_string(),
        // a non empty vec names its element, an empty one goes through the bare `Vec` key
        Value::Vec(items) => match items.lock().first() {
            Some(first) => format!("Vec<{}>", source_type_name(first)),
            None => "Vec".to_string(),
        },
        Value::Map(_, super::value::MapKind::Map) => "HashMap".to_string(),
        Value::Map(_, super::value::MapKind::Set) => "HashSet".to_string(),
        Value::Native(n) => match &*n.lock() {
            super::native::Native::ParseErr { debug, .. } => {
                debug.split(' ').next().unwrap_or("").to_string()
            }
            super::native::Native::IoErr { .. } => "Error".to_string(),
            other => other.type_name().to_string(),
        },
        other => other.type_name().to_string(),
    }
}
