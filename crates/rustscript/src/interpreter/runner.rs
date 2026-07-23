use std::cell::RefCell;
use std::mem::take;
use std::rc::Rc;

use anyhow::{Result, bail};

use super::Interp;
use super::bytecode::Chunk;
use super::value::{ClosureData, Upvalue, Value};

thread_local! {
    static STACK_POOL: RefCell<Vec<Vec<Value>>> = const { RefCell::new(Vec::new()) };
}

fn take_stack() -> Vec<Value> {
    STACK_POOL
        .with(|pool| pool.borrow_mut().pop())
        .unwrap_or_default()
}

fn recycle_stack(mut stack: Vec<Value>) {
    stack.clear();
    STACK_POOL.with(|pool| {
        let mut pool = pool.borrow_mut();
        if pool.len() < 32 {
            pool.push(stack);
        }
    });
}

pub(super) struct ClosureRunner<'a> {
    interpreter: &'a Interp,
    closure: Rc<ClosureData>,
    stack: Vec<Value>,
}

impl Interp {
    pub(super) fn run_chunk(
        &self,
        chunk: &Rc<Chunk>,
        args: &[Value],
        upvalues: &[Upvalue],
    ) -> Result<Value> {
        check_arity(chunk, args.len())?;
        let mut stack = take_stack();
        prepare_stack(&mut stack, chunk, args.iter());
        let result = self.exec(chunk, &mut stack, upvalues);
        recycle_stack(stack);
        result
    }

    pub(super) fn call_closure(&self, closure: &ClosureData, args: &[Value]) -> Result<Value> {
        self.run_chunk(&closure.chunk, args, &closure.captured)
    }

    pub(super) fn closure_runner(&self, closure: Rc<ClosureData>) -> ClosureRunner<'_> {
        ClosureRunner {
            interpreter: self,
            closure,
            stack: take_stack(),
        }
    }
}

impl ClosureRunner<'_> {
    pub(super) fn call_refs(&mut self, args: &[&Value]) -> Result<Value> {
        let chunk = &self.closure.chunk;
        check_arity(chunk, args.len())?;
        prepare_stack(&mut self.stack, chunk, args.iter().copied());
        self.interpreter
            .exec(chunk, &mut self.stack, &self.closure.captured)
    }
}

impl Drop for ClosureRunner<'_> {
    fn drop(&mut self) {
        recycle_stack(take(&mut self.stack));
    }
}

fn check_arity(chunk: &Chunk, actual: usize) -> Result<()> {
    if actual != chunk.num_params {
        bail!(
            "`{}` expects {} args but got {actual}",
            chunk.name,
            chunk.num_params
        );
    }
    Ok(())
}

fn prepare_stack<'a>(stack: &mut Vec<Value>, chunk: &Chunk, args: impl Iterator<Item = &'a Value>) {
    stack.clear();
    stack.resize(chunk.num_regs.max(chunk.num_params), Value::Unit);
    for (slot, argument) in stack.iter_mut().zip(args) {
        *slot = argument.clone();
    }
}
