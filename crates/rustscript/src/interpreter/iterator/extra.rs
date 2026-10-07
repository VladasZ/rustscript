//! `scan`, `map_while`, `cycle`, `next_if`, `min_by`, `max_by`, `unzip` and `is_sorted`.

use std::cmp::Ordering;
use std::slice::from_ref;
use std::sync::Arc;

use anyhow::{Result, bail};
use parking_lot::Mutex;

use super::{Handle, IteratorState, as_closure, owns_items, wrap};
use crate::interpreter::bridge::arg;
use crate::interpreter::bytecode::BuiltinId;
use crate::interpreter::methods::ordering_from_value;
use crate::interpreter::native::Native;
use crate::interpreter::ops::partial_compare;
use crate::interpreter::shared::usize_value;
use crate::interpreter::value::{ClosureData, Value, ValueRef};
use crate::interpreter::vm::Vm;

fn fork_handle(handle: &Handle) -> Result<Handle> {
    let state = match &*handle.lock() {
        Native::Iterator(state) => state.fork()?,
        other => bail!("{} can not be cloned for `cycle`", other.type_name()),
    };
    Ok(Arc::new(Mutex::new(Native::Iterator(state))))
}

impl IteratorState {
    /// What `Clone` gives the iterator in real Rust. An owning iterator clones the items it
    /// still holds, a borrowing one shares the collection.
    fn fork(&self) -> Result<IteratorState> {
        Ok(match self {
            IteratorState::Values {
                values,
                index,
                owned: true,
                back,
            } => {
                let items = values.lock();
                let end = items.len().saturating_sub(*back);
                let rest = items[(*index).min(end)..end]
                    .iter()
                    .map(Value::deep_clone)
                    .collect();
                IteratorState::Owned {
                    values: rest,
                    index: 0,
                    vec: true,
                }
            }
            IteratorState::Values {
                values,
                index,
                owned,
                back,
            } => IteratorState::Values {
                values: values.clone(),
                index: *index,
                owned: *owned,
                back: *back,
            },
            IteratorState::Owned { values, index, vec } => IteratorState::Owned {
                values: values[(*index).min(values.len())..]
                    .iter()
                    .map(|item| {
                        if *vec {
                            item.deep_clone()
                        } else {
                            item.clone()
                        }
                    })
                    .collect(),
                index: 0,
                vec: *vec,
            },
            IteratorState::UserNext { value } => IteratorState::UserNext {
                value: value.deep_clone(),
            },
            IteratorState::Range {
                next,
                end,
                inclusive,
                width,
            } => IteratorState::Range {
                next: *next,
                end: *end,
                inclusive: *inclusive,
                width: *width,
            },
            IteratorState::Repeat { value, remaining } => IteratorState::Repeat {
                value: value.deep_clone(),
                remaining: *remaining,
            },
            IteratorState::MutableValues { .. } | IteratorState::DrainingValues { .. } => {
                bail!("an iterator over `&mut` items can not be cloned for `cycle`")
            }
            other => other.fork_text()?,
        })
    }

    /// The iterators over a string, they share their source.
    fn fork_text(&self) -> Result<IteratorState> {
        Ok(match self {
            IteratorState::Bytes { source, index } => IteratorState::Bytes {
                source: source.clone(),
                index: *index,
            },
            IteratorState::Chars {
                source,
                offset,
                back,
            } => IteratorState::Chars {
                source: source.clone(),
                offset: *offset,
                back: *back,
            },
            IteratorState::Lines { source, offset } => IteratorState::Lines {
                source: source.clone(),
                offset: *offset,
            },
            IteratorState::SplitWhitespace { source, offset } => IteratorState::SplitWhitespace {
                source: source.clone(),
                offset: *offset,
            },
            IteratorState::RegexFind {
                regex,
                source,
                offset,
            } => IteratorState::RegexFind {
                regex: regex.clone(),
                source: source.clone(),
                offset: *offset,
            },
            IteratorState::RegexCaptures {
                regex,
                source,
                offset,
            } => IteratorState::RegexCaptures {
                regex: regex.clone(),
                source: source.clone(),
                offset: *offset,
            },
            adapter => adapter.fork_adapter()?,
        })
    }

    fn fork_adapter(&self) -> Result<IteratorState> {
        Ok(match self {
            IteratorState::Zip { left, right } => IteratorState::Zip {
                left: fork_handle(left)?,
                right: fork_handle(right)?,
            },
            IteratorState::Chain {
                left,
                right,
                left_done,
            } => IteratorState::Chain {
                left: fork_handle(left)?,
                right: fork_handle(right)?,
                left_done: *left_done,
            },
            IteratorState::Map { source, closure } => IteratorState::Map {
                source: fork_handle(source)?,
                closure: closure.clone(),
            },
            IteratorState::Filter { source, closure } => IteratorState::Filter {
                source: fork_handle(source)?,
                closure: closure.clone(),
            },
            IteratorState::FilterMap { source, closure } => IteratorState::FilterMap {
                source: fork_handle(source)?,
                closure: closure.clone(),
            },
            IteratorState::Inspect { source, closure } => IteratorState::Inspect {
                source: fork_handle(source)?,
                closure: closure.clone(),
            },
            IteratorState::Enumerate { source, index } => IteratorState::Enumerate {
                source: fork_handle(source)?,
                index: *index,
            },
            IteratorState::Take { source, remaining } => IteratorState::Take {
                source: fork_handle(source)?,
                remaining: *remaining,
            },
            IteratorState::Cloned { source } => IteratorState::Cloned {
                source: fork_handle(source)?,
            },
            IteratorState::Skip { source, remaining } => IteratorState::Skip {
                source: fork_handle(source)?,
                remaining: *remaining,
            },
            IteratorState::Rev { source } => IteratorState::Rev {
                source: fork_handle(source)?,
            },
            IteratorState::StepBy {
                source,
                step,
                first,
            } => IteratorState::StepBy {
                source: fork_handle(source)?,
                step: *step,
                first: *first,
            },
            IteratorState::TakeWhile {
                source,
                closure,
                done,
            } => IteratorState::TakeWhile {
                source: fork_handle(source)?,
                closure: closure.clone(),
                done: *done,
            },
            IteratorState::SkipWhile {
                source,
                closure,
                skipping,
            } => IteratorState::SkipWhile {
                source: fork_handle(source)?,
                closure: closure.clone(),
                skipping: *skipping,
            },
            IteratorState::Peekable { source, buffered } => IteratorState::Peekable {
                source: fork_handle(source)?,
                buffered: buffered.as_ref().map(Value::deep_clone),
            },
            IteratorState::Scan {
                source,
                closure,
                state,
            } => IteratorState::Scan {
                source: fork_handle(source)?,
                closure: closure.clone(),
                state: Arc::new(Mutex::new(state.lock().deep_clone())),
            },
            IteratorState::MapWhile { source, closure } => IteratorState::MapWhile {
                source: fork_handle(source)?,
                closure: closure.clone(),
            },
            IteratorState::Cycle { original, current } => IteratorState::Cycle {
                original: fork_handle(original)?,
                current: fork_handle(current)?,
            },
            _ => unreachable!("fork handles the sources itself"),
        })
    }
}

fn exact_len_of(handle: &Handle) -> Option<usize> {
    match &*handle.lock() {
        Native::Iterator(state) => state.exact_len(),
        _ => None,
    }
}

impl IteratorState {
    /// The items left, for the iterators std gives an exact size. `None` for the rest.
    fn exact_len(&self) -> Option<usize> {
        match self {
            IteratorState::Values {
                values,
                index,
                back,
                ..
            } => Some(values.lock().len().saturating_sub(*index + *back)),
            IteratorState::MutableValues { values, index } => {
                Some(values.lock().len().saturating_sub(*index))
            }
            IteratorState::DrainingValues { values } => Some(values.lock().len()),
            IteratorState::Owned { values, index, .. } => Some(values.len().saturating_sub(*index)),
            IteratorState::Range {
                next,
                end,
                inclusive,
                ..
            } => {
                let end = end.saturating_add(i64::from(*inclusive));
                usize::try_from(end.saturating_sub(*next).max(0)).ok()
            }
            IteratorState::Bytes { source, index } => Some(source.len().saturating_sub(*index)),
            IteratorState::Map { source, .. }
            | IteratorState::Inspect { source, .. }
            | IteratorState::Enumerate { source, .. }
            | IteratorState::Cloned { source }
            | IteratorState::Rev { source } => exact_len_of(source),
            IteratorState::Take { source, remaining } => {
                Some(exact_len_of(source)?.min(*remaining))
            }
            IteratorState::Skip { source, remaining } => {
                Some(exact_len_of(source)?.saturating_sub(*remaining))
            }
            IteratorState::StepBy {
                source,
                step,
                first,
            } => {
                let left = exact_len_of(source)?;
                Some(if *first {
                    left.div_ceil(*step)
                } else {
                    left / *step
                })
            }
            IteratorState::Zip { left, right } => {
                Some(exact_len_of(left)?.min(exact_len_of(right)?))
            }
            IteratorState::Peekable { source, buffered } => {
                Some(exact_len_of(source)? + usize::from(buffered.is_some()))
            }
            _ => None,
        }
    }
}

impl Vm {
    /// std does not fuse a `scan`, a pull after its `None` calls the closure again.
    pub(super) fn scan_next(
        self: &Arc<Self>,
        source: &Handle,
        closure: &Arc<ClosureData>,
        state: Arc<Mutex<Value>>,
    ) -> Result<Option<Value>> {
        let Some(value) = self.iterator_next(source)? else {
            return Ok(None);
        };
        let owned = owns_items(source);
        let state = Value::Ref(Arc::new(ValueRef::cell_slot(state)));
        Ok(self
            .call_closure_with(closure, &[state, value], owned)?
            .some_payload())
    }

    pub(super) fn map_while_next(
        self: &Arc<Self>,
        source: &Handle,
        closure: &Arc<ClosureData>,
    ) -> Result<Option<Value>> {
        let Some(value) = self.iterator_next(source)? else {
            return Ok(None);
        };
        let owned = owns_items(source);
        Ok(self
            .call_closure_with(closure, &[value], owned)?
            .some_payload())
    }

    /// A round that ends starts the next one from a fresh copy of the original. An empty
    /// original ends the cycle.
    pub(super) fn cycle_next(
        self: &Arc<Self>,
        iterator: &Handle,
        original: &Handle,
        current: &Handle,
    ) -> Result<Option<Value>> {
        if let Some(value) = self.iterator_next(current)? {
            return Ok(Some(value));
        }
        // the round that ended drops what it still holds as the next one takes its place
        self.drop_leftovers(current)?;
        let fresh = fork_handle(original)?;
        let first = self.iterator_next(&fresh)?;
        if let Native::Iterator(IteratorState::Cycle { current, .. }) = &mut *iterator.lock() {
            *current = fresh;
        }
        Ok(first)
    }

    /// The adapters and terminals of this file. `None` for any other method.
    pub(in crate::interpreter) fn run_extra_method(
        self: &Arc<Self>,
        iterator: &Handle,
        id: BuiltinId,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let closure = |index| as_closure(args.get(index));
        Ok(Some(match id {
            BuiltinId::Scan => wrap(IteratorState::Scan {
                source: iterator.clone(),
                closure: closure(1)?,
                state: Arc::new(Mutex::new(arg(args, 0)?)),
            }),
            BuiltinId::MapWhile => wrap(IteratorState::MapWhile {
                source: iterator.clone(),
                closure: closure(0)?,
            }),
            BuiltinId::Cycle => wrap(IteratorState::Cycle {
                original: fork_handle(iterator)?,
                current: iterator.clone(),
            }),
            BuiltinId::NextIf => {
                let closure = closure(0)?;
                let keep = |vm: &Arc<Self>, item: &Value| {
                    Ok(vm.call_closure_data(&closure, from_ref(item))?.is_truthy())
                };
                match self.next_if(iterator, keep)? {
                    Some(value) => value,
                    None => return Ok(None),
                }
            }
            BuiltinId::NextIfEq => {
                let wanted = arg(args, 0)?;
                let keep = |_: &Arc<Self>, item: &Value| Ok(item.eq_value(&wanted));
                match self.next_if(iterator, keep)? {
                    Some(value) => value,
                    None => return Ok(None),
                }
            }
            BuiltinId::MaxBy | BuiltinId::MinBy => self.extreme_by(iterator, id, &closure(0)?)?,
            BuiltinId::Len => match exact_len_of(iterator) {
                Some(len) => usize_value(len),
                None => return Ok(None),
            },
            BuiltinId::Unzip => self.unzip(iterator)?,
            BuiltinId::IsSorted => self.is_sorted(iterator)?,
            _ => return Ok(None),
        }))
    }

    /// `None` when the handle is not a peekable.
    fn next_if(
        self: &Arc<Self>,
        iterator: &Handle,
        keep: impl Fn(&Arc<Self>, &Value) -> Result<bool>,
    ) -> Result<Option<Value>> {
        let Some(peeked) = self.peek(iterator)? else {
            return Ok(None);
        };
        let Some(item) = peeked.some_payload() else {
            return Ok(Some(Value::none()));
        };
        if !keep(self, &item)? {
            return Ok(Some(Value::none()));
        }
        if let Native::Iterator(IteratorState::Peekable { buffered, .. }) = &mut *iterator.lock() {
            *buffered = None;
        }
        Ok(Some(Value::some(item)))
    }

    /// std folds with the best so far on the left. `max_by` keeps the later of 2 equal items
    /// and `min_by` the earlier one.
    fn extreme_by(
        self: &Arc<Self>,
        iterator: &Handle,
        name: BuiltinId,
        closure: &Arc<ClosureData>,
    ) -> Result<Value> {
        let mut best: Option<Value> = None;
        while let Some(value) = self.iterator_next(iterator)? {
            let Some(current) = best.take() else {
                best = Some(value);
                continue;
            };
            let order = match self.call_closure_data(closure, &[current.clone(), value.clone()]) {
                Ok(order) => ordering_from_value(&order).unwrap_or(Ordering::Equal),
                Err(error) => {
                    // the 2 arguments of the fold unwind in reverse
                    for item in [value, current] {
                        if let Err(error) = self.discard(iterator, item) {
                            eprintln!("panic in drop during unwinding: {error:#}");
                        }
                    }
                    return Err(error);
                }
            };
            let keep_current = if name == BuiltinId::MaxBy {
                order == Ordering::Greater
            } else {
                order != Ordering::Greater
            };
            let (winner, loser) = if keep_current {
                (current, value)
            } else {
                (value, current)
            };
            self.discard(iterator, loser)?;
            best = Some(winner);
        }
        Ok(best.map_or_else(Value::none, Value::some))
    }

    fn unzip(self: &Arc<Self>, iterator: &Handle) -> Result<Value> {
        let (mut left, mut right) = (Vec::new(), Vec::new());
        while let Some(item) = self.iterator_next(iterator)? {
            let Value::Tuple(parts) = &item else {
                bail!("unzip needs pairs, got {}", item.type_name());
            };
            let mut parts = parts.lock().clone().into_iter();
            let (Some(first), Some(second), None) = (parts.next(), parts.next(), parts.next())
            else {
                bail!("unzip needs pairs");
            };
            left.push(first);
            right.push(second);
        }
        Ok(Value::tuple(vec![Value::vec(left), Value::vec(right)]))
    }

    fn is_sorted(self: &Arc<Self>, iterator: &Handle) -> Result<Value> {
        let mut sorted = true;
        let mut last: Option<Value> = None;
        while let Some(item) = self.iterator_next(iterator)? {
            if let Some(before) = last.replace(item.clone()) {
                let ordered = matches!(
                    partial_compare(&before, &item)?,
                    Some(Ordering::Less | Ordering::Equal)
                );
                self.discard(iterator, before)?;
                if !ordered {
                    sorted = false;
                    break;
                }
            }
        }
        if let Some(item) = last {
            self.discard(iterator, item)?;
        }
        Ok(Value::Bool(sorted))
    }
}
