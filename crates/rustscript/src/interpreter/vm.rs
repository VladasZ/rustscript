//! The frame loop. `exec` owns the call frames and applies the `Flow` each op returns. The op
//! bodies are in `vm_step`.

use std::cell::Cell;
use std::collections::HashMap;

use rustc_hash::FxHashMap;
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
use super::vm_step::{CallReq, Flow, StepCtx, step};
use super::vm_support::{FrameSite, trace_error};

pub(super) const MAX_CALL_DEPTH: usize = 100_000;

/// The host stack of every thread that runs script code. A closure called from a native like
/// `map` nests a whole `exec` on the host stack, so the depth a script can reach through iterator
/// closures is bounded by this, not by `MAX_CALL_DEPTH` alone. Virtual memory, only the pages a
/// deep run touches are ever committed.
pub const SCRIPT_STACK_BYTES: usize = 1 << 30;

/// Deeper nesting gives buffers back to the allocator, so a burst of recursion doesn't pin its
/// memory forever.
const MAX_POOLED_STACKS: usize = 32;

thread_local! {
    /// 1 popped per live call on this thread
    static STACK_POOL: std::cell::RefCell<Vec<Vec<Value>>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// Live `run_chunk` nestings on this thread. Each one is a native frame set on the host
    /// stack, so they count toward the depth cap like VM frames do.
    static NESTED: Cell<usize> = const { Cell::new(0) };
}

/// Below this much host stack the next nesting stops with the script panic. Otherwise a debug
/// build, whose native frames are several times larger, aborts the process before the depth cap.
const STACK_RED_ZONE: usize = 256 * 1024;

/// Holds one nesting level for the life of a `run_chunk` call.
struct Nesting;

impl Nesting {
    fn enter() -> Result<Self> {
        let depth = NESTED.with(Cell::get);
        if depth >= MAX_CALL_DEPTH {
            bail!("stack overflow: call depth exceeded {MAX_CALL_DEPTH}");
        }
        if stacker::remaining_stack().is_some_and(|left| left < STACK_RED_ZONE) {
            bail!("stack overflow: host stack exhausted after {depth} nested calls");
        }
        NESTED.with(|n| n.set(depth + 1));
        Ok(Self)
    }

    fn depth() -> usize {
        NESTED.with(Cell::get)
    }
}

impl Drop for Nesting {
    fn drop(&mut self) {
        NESTED.with(|n| n.set(n.get() - 1));
    }
}

fn swap_option<T>(current: &mut Option<T>, next: Option<T>) -> Option<T> {
    match next {
        Some(value) => current.replace(value),
        None => current.take(),
    }
}

fn frame_line(chunk: &Chunk, ip: usize) -> FrameSite {
    FrameSite {
        func: chunk.name.clone(),
        file: chunk.file.to_string(),
        line: chunk.lines.get(ip).copied().unwrap_or(0),
        col: chunk.cols.get(ip).copied().unwrap_or(0),
    }
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
    /// Some script type has a `Drop` impl, so a store drops what it overwrites. Without one no
    /// drop can print, and the containers can just be released.
    pub has_drop: bool,
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
        Value::structure(
            StructShape::typed(name, type_id, fields, Vec::new(), Vec::new()),
            args,
        )
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

/// How a run of ops in one frame ended.
enum Exit {
    Ret(Value),
    Call(CallReq),
}

/// Where execution is, the frame being run.
struct Cursor {
    chunk: Arc<Chunk>,
    closure: Option<Arc<ClosureData>>,
    type_env: TypeEnv,
    base: usize,
    ip: usize,
    /// see `Op::DropParams`
    owned_args: bool,
}

/// Sets up the callee's registers and returns the caller's frame to come back to.
fn enter(at: &mut Cursor, stack: &mut Vec<Value>, req: CallReq) -> Frame {
    let nbase = at.base + at.chunk.num_regs;
    let need = nbase + req.chunk.num_regs.max(req.chunk.num_params);
    if stack.len() < need {
        stack.resize(need, Value::Unit);
    }
    for i in 0..req.argc {
        stack[nbase + i] = take(&mut stack[at.base + req.abase + i]);
    }
    for slot in &mut stack[nbase + req.argc..need] {
        *slot = Value::Unit;
    }
    let frame = Frame {
        chunk: replace(&mut at.chunk, req.chunk),
        closure: swap_option(&mut at.closure, req.closure),
        ip: at.ip + 1,
        base: at.base,
        dst: req.dst,
        abase: u16::try_from(req.abase).expect("register index fits u16"),
        argc: u16::try_from(req.argc).expect("argument count fits u16"),
        type_env: replace(&mut at.type_env, req.type_env),
        owned_args: replace(&mut at.owned_args, req.owned_args),
    };
    at.base = nbase;
    at.ip = 0;
    frame
}

/// Comes back to `frame` with the callee's result in its destination register.
fn leave(
    at: &mut Cursor,
    local_cells: &mut FxHashMap<usize, Arc<Mutex<Value>>>,
    stack: &mut [Value],
    frame: Frame,
    result: Value,
) {
    let callee_base = at.base;
    let callee_end = callee_base + at.chunk.num_regs;
    let clears_frame = at.chunk.clears_frame;
    local_cells.retain(|slot, _| *slot < callee_base || *slot >= callee_end);
    at.chunk = frame.chunk;
    at.closure = frame.closure;
    at.type_env = frame.type_env;
    at.ip = frame.ip;
    at.base = frame.base;
    at.owned_args = frame.owned_args;
    // the `&mut` argument writeback picks these up from the caller's arg window
    for i in 0..frame.argc as usize {
        stack[at.base + frame.abase as usize + i] = take(&mut stack[callee_base + i]);
    }
    // a guard left in a dead frame would still count as a live borrow
    if clears_frame {
        for slot in &mut stack[callee_base + frame.argc as usize..callee_end] {
            *slot = Value::Unit;
        }
    }
    stack[at.base + frame.dst as usize] = result;
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
    /// see `Op::DropParams`
    owned_args: bool,
}

impl Vm {
    /// `owned_args` says whether the callee may drop its by value parameters at its end, see
    /// `Op::DropParams`. A native adapter over a borrowing iterator passes false.
    pub fn run_chunk(
        self: &Arc<Self>,
        chunk: &Arc<Chunk>,
        args: &[Value],
        upvalues: &[Upvalue],
        owned_args: bool,
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
        let nesting = Nesting::enter()?;
        // A fresh stack per call was the main cost in comparator sorts. Nested calls each pop
        // their own buffer.
        let mut stack = STACK_POOL
            .with(|pool| pool.borrow_mut().pop())
            .unwrap_or_default();
        let regs = chunk.num_regs.max(chunk.num_params);
        // a pooled buffer comes back at its old length with every heap handle released
        if stack.len() < regs {
            stack.resize(regs, Value::Unit);
        }
        for (slot, a) in stack.iter_mut().zip(args) {
            *slot = a.clone();
        }
        for slot in &mut stack[args.len()..regs] {
            *slot = Value::Unit;
        }
        let result = self.exec(chunk, &mut stack, upvalues, owned_args);
        drop(nesting);
        for slot in &mut stack {
            slot.release();
        }
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
        owned_args: bool,
    ) -> Result<Value> {
        let mut frames: Vec<Frame> = Vec::new();
        let mut local_cells: FxHashMap<usize, Arc<Mutex<Value>>> = FxHashMap::default();
        let mut at = Cursor {
            chunk: entry.clone(),
            closure: None,
            type_env: empty_type_env(),
            base: 0,
            ip: 0,
            owned_args,
        };
        // One immediately called closure, so an error can be annotated with the call chain still in
        // `frames` and the failing op in `at`.
        let result = (|| -> Result<Value> {
            loop {
                match self.run_frame(&mut at, &mut local_cells, stack, entry_upvalues)? {
                    Exit::Ret(v) => {
                        let Some(frame) = frames.pop() else {
                            return Ok(v);
                        };
                        leave(&mut at, &mut local_cells, stack, frame, v);
                    }
                    Exit::Call(req) => {
                        if Nesting::depth() + frames.len() >= MAX_CALL_DEPTH {
                            bail!("stack overflow: call depth exceeded {MAX_CALL_DEPTH}");
                        }
                        frames.push(enter(&mut at, stack, req));
                    }
                }
            }
        })();
        result.map_err(|e| {
            self.unwind_drops(&at.chunk, at.base, at.owned_args, &frames, stack);
            let trace = std::iter::once(frame_line(&at.chunk, at.ip)).chain(
                frames
                    .iter()
                    .rev()
                    .map(|f| frame_line(&f.chunk, f.ip.saturating_sub(1))),
            );
            trace_error(e, trace)
        })
    }

    /// Runs the ops of the current frame until it returns or calls. One context serves the whole
    /// run, it is rebuilt only when the frame changes.
    fn run_frame(
        self: &Arc<Self>,
        at: &mut Cursor,
        local_cells: &mut FxHashMap<usize, Arc<Mutex<Value>>>,
        stack: &mut Vec<Value>,
        entry_upvalues: &[Upvalue],
    ) -> Result<Exit> {
        let mut ctx = StepCtx {
            vm: self,
            cur: &at.chunk,
            cur_clo: &at.closure,
            cur_tenv: &at.type_env,
            entry_upvalues,
            local_cells,
            stack,
            base: at.base,
            ip: at.ip,
            ret: Value::Unit,
            call: None,
            owned_args: at.owned_args,
        };
        let exit = loop {
            let Some(op) = ctx.cur.code.get(ctx.ip) else {
                break Ok(Exit::Ret(Value::Unit));
            };
            match step(&mut ctx, op) {
                Ok(Flow::Next) => ctx.ip += 1,
                Ok(Flow::Jump(to)) => ctx.ip = to,
                Ok(Flow::Ret) => break Ok(Exit::Ret(take(&mut ctx.ret))),
                Ok(Flow::Call) => {
                    break Ok(Exit::Call(
                        ctx.call.take().expect("a call op filled the request"),
                    ));
                }
                Err(e) => break Err(e),
            }
        };
        at.ip = ctx.ip;
        exit
    }

    /// Innermost frame first and highest register first, like real unwinding. A panic inside a drop is
    /// reported and the original panic keeps going. Real Rust would abort there.
    fn unwind_drops(
        self: &Arc<Self>,
        cur: &Arc<Chunk>,
        base: usize,
        owned_args: bool,
        frames: &[Frame],
        stack: &mut [Value],
    ) {
        let spans = std::iter::once((base, cur, owned_args)).chain(
            frames
                .iter()
                .rev()
                .map(|f| (f.base, &f.chunk, f.owned_args)),
        );
        for (base, chunk, owned) in spans {
            for &reg in chunk.droppable.iter() {
                // a lent parameter belongs to the caller
                if !owned && chunk.lent_params.contains(&reg) {
                    continue;
                }
                let Some(slot) = stack.get_mut(base + usize::from(reg)) else {
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
        let v = self.run_chunk(&chunk, &[], &[], true)?;
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

    /// Stores the field and hands the old value back for its drop.
    pub(super) fn set_field(recv: &Value, member: &Member, v: Value) -> Result<Value> {
        let old = match (recv, member) {
            (Value::Struct(s), Member::Named(n)) => {
                let Some(i) = n.slot_in(&s.shape) else {
                    bail!("no field `{n}`");
                };
                std::mem::replace(&mut s.values.lock()[i], v)
            }
            (Value::Tuple(t), Member::Indexed(i)) => {
                let mut t = t.lock();
                if *i >= t.len() {
                    bail!("no tuple index {i}");
                }
                std::mem::replace(&mut t[*i], v)
            }
            // a tuple struct names its fields by position
            (Value::Struct(s), Member::Indexed(i)) => {
                let mut values = s.values.lock();
                if *i >= values.len() {
                    bail!("no field {i}");
                }
                std::mem::replace(&mut values[*i], v)
            }
            _ => bail!("cannot set a field of {}", recv.type_name()),
        };
        Ok(old)
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
