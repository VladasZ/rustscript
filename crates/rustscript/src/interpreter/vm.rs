//! The register machine frame loop. It runs `Chunk` over `Value`, so a task
//! can run on any worker thread. `exec` owns the call frames and applies the
//! `Flow` each dispatched op answers with; the op bodies live in `vm_step`.

use std::collections::HashMap;
use std::mem::{replace, take};
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use parking_lot::Mutex;
use tokio::runtime::Handle;

use super::bytecode::{Chunk, Member, path_call_chunk};
use super::native::Native;
use super::typeir::TypeIr;
use super::value::{ClosureData, StructShape, Upvalue, Value};
use super::vm_step::{Flow, StepCtx, step};
use super::vm_support::trace_error;

pub(super) const MAX_CALL_DEPTH: usize = 100_000;

/// Deeper nesting than this returns buffers to the allocator, so a burst of
/// recursion does not pin its high-water memory forever.
const MAX_POOLED_STACKS: usize = 32;

thread_local! {
    /// Reusable value stacks for `run_chunk`, one popped per live call on
    /// this thread.
    static STACK_POOL: std::cell::RefCell<Vec<Vec<Value>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

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
                return Some(Value::enum_of(def.name.clone(), variant, Vec::new()));
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
                return Some(Value::enum_of(def.name.clone(), variant, args.to_vec()));
            }
        }
        None
    }

    pub(super) fn make_tuple_struct(name: &str, args: Vec<Value>) -> Value {
        let fields = (0..args.len()).map(|i| i.to_string().into()).collect();
        Value::structure(StructShape::new(name, fields), args)
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
}

/// A binding of a generic parameter name to the lowered concrete type a
/// caller passed by turbofish.
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
        // A path forwarder's arity is only a guess, so rebuild it for the
        // count actually passed. `u8::saturating_add` handed to `fold` takes
        // two arguments where the guess was one.
        let rebuilt;
        let chunk = if chunk.path_forwarder && args.len() != chunk.num_params {
            rebuilt = path_call_chunk(chunk.paths[0].0.clone(), args.len());
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
        // A closure called per element allocated a fresh stack per call,
        // which dominated comparator sorts. The pool hands the buffer back
        // for the next call on this thread instead. Nested calls each pop
        // their own buffer, so re-entrancy just deepens the pool.
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

        // The dispatch runs inside one immediately called closure so an error
        // can be annotated with the script call chain still held in `frames`
        // and the failing op still addressed by `cur` and `ip`. The closure
        // runs exactly once, so the hot loop itself is unchanged.
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
                        // The callee's final parameter values go back into the
                        // caller's arg window, where a `&mut` argument
                        // writeback emitted by the compiler picks them up.
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

    /// Run user `Drop` impls for every live local of every frame being
    /// unwound by a panic, innermost frame first and highest register first,
    /// the way real Rust unwinding drops locals in reverse declaration
    /// order. A register whose storage has another holder was moved or is
    /// still shared, so its real owner drops it, exactly like a scope-end
    /// drop. A panic inside one of these drops cannot unwind again, real
    /// Rust would abort there, so it is reported and the original panic
    /// keeps propagating.
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

    /// Drive an awaited value to its result. A `JoinHandle` joins, a future is
    /// run to completion, anything else is already a value.
    pub(super) fn await_value(&self, v: Value) -> Result<Value> {
        let Value::Native(n) = v else { return Ok(v) };
        let taken = replace(&mut *n.lock(), Native::Taken);
        match taken {
            // Awaiting a JoinHandle yields `Result<T, JoinError>` in real Rust,
            // so it wraps. A script that passes `rust check` writes `.await?` or
            // `.await.unwrap()`, and both need the Ok layer to be here.
            Native::Task(h) => Ok(match self.rt.block_on(h) {
                Ok(v) => Value::ok(v),
                // The real JoinError renders both its format forms here, so
                // `{e:?}` prints the same `JoinError::Panic(Id(11), "boom",
                // ...)` a compiled binary prints.
                Err(e) => Value::err(
                    Native::JoinErr {
                        display: e.to_string(),
                        debug: format!("{e:?}"),
                        is_panic: e.is_panic(),
                    }
                    .wrap(),
                ),
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

    /// Whether the value's user type declares this method itself.
    pub(super) fn user_method_exists(&self, recv: &Value, name: &str) -> bool {
        let ty = match recv {
            Value::Struct(s) => &**s.name(),
            Value::Enum { enum_name, .. } => &**enum_name,
            _ => return false,
        };
        self.methods
            .contains_key(&(ty.to_string(), name.to_string()))
    }

    pub(super) fn user_method(&self, ty: &str, name: &str) -> Option<Arc<Chunk>> {
        self.methods
            .get(&(ty.to_string(), name.to_string()))
            .cloned()
    }

    /// Value of a module const or static, evaluated on first read and cached.
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

/// A field access on a struct or tuple.
impl Vm {
    pub(super) fn get_field(recv: &Value, member: &Member) -> Result<Value> {
        match (recv, member) {
            (Value::Struct(s), Member::Named(n)) => {
                s.get(n).ok_or_else(|| anyhow!("no field `{n}`"))
            }
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
