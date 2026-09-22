//! The closure taking iterator methods and the reductions.

use std::slice::from_ref;
use std::sync::Arc;

use anyhow::Result;

use super::drive::consumes_iterator;
use super::{
    Handle, IteratorState, Reducer, as_closure, option_inner, owns_items, product_reducer,
    sum_reducer, wrap,
};
use crate::interpreter::bridge::arg;
use crate::interpreter::bytecode::{BuiltinId, ScalarTy};
use crate::interpreter::ops::compare_values;
use crate::interpreter::value::{ClosureData, Value};
use crate::interpreter::vm::Vm;

impl Vm {
    pub(in crate::interpreter) fn iterator_higher_order(
        self: &Arc<Self>,
        iterator: &Handle,
        name: BuiltinId,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let closure = |index| as_closure(args.get(index));
        let value = match name {
            BuiltinId::Map => wrap(IteratorState::Map {
                source: iterator.clone(),
                closure: closure(0)?,
            }),
            BuiltinId::Filter => wrap(IteratorState::Filter {
                source: iterator.clone(),
                closure: closure(0)?,
            }),
            BuiltinId::FilterMap => wrap(IteratorState::FilterMap {
                source: iterator.clone(),
                closure: closure(0)?,
            }),
            BuiltinId::Inspect => wrap(IteratorState::Inspect {
                source: iterator.clone(),
                closure: closure(0)?,
            }),
            BuiltinId::TakeWhile => wrap(IteratorState::TakeWhile {
                source: iterator.clone(),
                closure: closure(0)?,
                done: false,
            }),
            BuiltinId::SkipWhile => wrap(IteratorState::SkipWhile {
                source: iterator.clone(),
                closure: closure(0)?,
                skipping: true,
            }),
            BuiltinId::ForEach => {
                let closure = closure(0)?;
                let owned = owns_items(iterator);
                while let Some(value) = self.iterator_next(iterator)? {
                    self.call_closure_with(&closure, &[value], owned)?;
                }
                Value::Unit
            }
            BuiltinId::FindMap => {
                let closure = closure(0)?;
                let mut found = Value::none();
                while let Some(value) = self.iterator_next(iterator)? {
                    if let Some(inner) = option_inner(&self.call_closure_data(&closure, &[value])?)
                    {
                        found = Value::some(inner);
                        break;
                    }
                }
                found
            }
            BuiltinId::Find
            | BuiltinId::Position
            | BuiltinId::Rposition
            | BuiltinId::Any
            | BuiltinId::All => {
                let closure = closure(0)?;
                return self.iterator_predicate(iterator, name, &closure).map(Some);
            }
            _ => return self.iterator_reduce_ho(iterator, name, args),
        };
        if consumes_iterator(name) {
            self.drop_leftovers(iterator)?;
        }
        Ok(Some(value))
    }

    pub(super) fn iterator_reduce_ho(
        self: &Arc<Self>,
        iterator: &Handle,
        name: BuiltinId,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let closure = |index| as_closure(args.get(index));
        let value = match name {
            BuiltinId::Fold => {
                let closure = closure(1)?;
                let owned = owns_items(iterator);
                let mut accumulator = arg(args, 0)?;
                while let Some(value) = self.iterator_next(iterator)? {
                    accumulator = self.call_closure_with(&closure, &[accumulator, value], owned)?;
                }
                accumulator
            }
            BuiltinId::Reduce => {
                let closure = closure(0)?;
                let owned = owns_items(iterator);
                let Some(mut accumulator) = self.iterator_next(iterator)? else {
                    return Ok(Some(Value::none()));
                };
                while let Some(value) = self.iterator_next(iterator)? {
                    accumulator = self.call_closure_with(&closure, &[accumulator, value], owned)?;
                }
                Value::some(accumulator)
            }
            BuiltinId::FlatMap => {
                let closure = closure(0)?;
                let owned = owns_items(iterator);
                let mut output = Vec::new();
                while let Some(value) = self.iterator_next(iterator)? {
                    let mapped = self.call_closure_with(&closure, &[value], owned)?;
                    output.extend(self.drain_items(mapped)?);
                }
                Value::vec(output)
            }
            // the flattened items stay the iterator's own when the source owned them, so a
            // `count` drops what it throws away
            BuiltinId::Flatten => {
                let owned = owns_items(iterator);
                let mut output = Vec::new();
                while let Some(value) = self.iterator_next(iterator)? {
                    output.extend(self.drain_items(value)?);
                }
                wrap(IteratorState::Owned {
                    values: output,
                    index: 0,
                    vec: owned,
                })
            }
            BuiltinId::Partition => self.partition(iterator, &closure(0)?)?,
            BuiltinId::MaxByKey | BuiltinId::MinByKey => {
                self.extreme_by_key(iterator, name, &closure(0)?)?
            }
            _ => return Ok(None),
        };
        if consumes_iterator(name) {
            self.drop_leftovers(iterator)?;
        }
        Ok(Some(value))
    }

    fn partition(self: &Arc<Self>, iterator: &Handle, closure: &Arc<ClosureData>) -> Result<Value> {
        let (mut yes, mut no) = (Vec::new(), Vec::new());
        let result = (|| -> Result<()> {
            while let Some(value) = self.iterator_next(iterator)? {
                if self.call_lending(iterator, closure, &value)?.is_truthy() {
                    yes.push(value);
                } else {
                    no.push(value);
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            // The fold unwinds first, then the two output locals in reverse order.
            if let Err(error) = self.drop_leftovers(iterator) {
                eprintln!("panic in drop during unwinding: {error:#}");
            }
            for value in no.into_iter().chain(yes) {
                self.unwind_discard(iterator, value);
            }
            return Err(error);
        }
        Ok(Value::tuple(vec![Value::vec(yes), Value::vec(no)]))
    }

    fn extreme_by_key(
        self: &Arc<Self>,
        iterator: &Handle,
        name: BuiltinId,
        closure: &Arc<ClosureData>,
    ) -> Result<Value> {
        let mut best: Option<(Value, Value)> = None;
        let result = (|| -> Result<()> {
            while let Some(value) = self.iterator_next(iterator)? {
                let key = self.call_lending(iterator, closure, &value)?;
                let take = match &best {
                    None => true,
                    Some((best_key, _)) => {
                        let order = compare_values(&key, best_key)?;
                        if name == BuiltinId::MaxByKey {
                            order.is_ge()
                        } else {
                            order.is_lt()
                        }
                    }
                };
                let loser = if take {
                    best.replace((key, value))
                } else {
                    Some((key, value))
                };
                if let Some((key, value)) = loser {
                    self.run_user_drop(key)?;
                    self.discard(iterator, value)?;
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            if let Some((key, value)) = best {
                if let Err(error) = self.run_user_drop(key) {
                    eprintln!("panic in drop during unwinding: {error:#}");
                }
                self.unwind_discard(iterator, value);
            }
            return Err(error);
        }
        Ok(match best {
            Some((key, value)) => {
                self.run_user_drop(key)?;
                Value::some(value)
            }
            None => Value::none(),
        })
    }

    fn unwind_discard(self: &Arc<Self>, iterator: &Handle, value: Value) {
        if let Err(error) = self.discard(iterator, value) {
            eprintln!("panic in drop during unwinding: {error:#}");
        }
    }

    pub(super) fn drain_iterator(self: &Arc<Self>, iterator: &Handle) -> Result<Vec<Value>> {
        let mut values = Vec::new();
        while let Some(value) = self.iterator_next(iterator)? {
            values.push(value);
        }
        Ok(values)
    }

    pub(super) fn iterator_sum(
        self: &Arc<Self>,
        iterator: &Handle,
        target: Option<&ScalarTy>,
    ) -> Result<Value> {
        self.reduce_iterator(iterator, sum_reducer(target))
    }

    pub(super) fn iterator_product(
        self: &Arc<Self>,
        iterator: &Handle,
        target: Option<&ScalarTy>,
    ) -> Result<Value> {
        self.reduce_iterator(iterator, product_reducer(target))
    }

    /// One element at a time, so a step that overflows the accumulator panics before the
    /// source produces the next element. A source with a side effect that panics on its own
    /// would otherwise report that later panic instead.
    fn reduce_iterator(
        self: &Arc<Self>,
        iterator: &Handle,
        mut reducer: Reducer<'_>,
    ) -> Result<Value> {
        while let Some(value) = self.iterator_next(iterator)? {
            reducer.push(&value)?;
        }
        Ok(reducer.finish())
    }

    pub(super) fn iterator_extreme(
        self: &Arc<Self>,
        iterator: &Handle,
        name: BuiltinId,
    ) -> Result<Value> {
        let mut best: Option<Value> = None;
        while let Some(value) = self.iterator_next(iterator)? {
            let take = match &best {
                None => true,
                Some(current) => {
                    let order = compare_values(&value, current)?;
                    // std keeps the last of equal maxima and the first of equal minima
                    if name == BuiltinId::Max {
                        order.is_ge()
                    } else {
                        order.is_lt()
                    }
                }
            };
            if take {
                if let Some(loser) = best.replace(value) {
                    self.discard(iterator, loser)?;
                }
            } else {
                self.discard(iterator, value)?;
            }
        }
        Ok(best.map_or_else(Value::none, Value::some))
    }

    pub(super) fn iterator_predicate(
        self: &Arc<Self>,
        iterator: &Handle,
        name: BuiltinId,
        closure: &Arc<ClosureData>,
    ) -> Result<Value> {
        let mut index = 0;
        // `rposition` walks to the end and keeps the last match, same index without a reversible
        // iterator
        let mut last_match = None;
        // `find` looks at each item and keeps the match, the others take theirs by value
        let by_value = name != BuiltinId::Find && owns_items(iterator);
        while let Some(value) = self.iterator_next(iterator)? {
            let matches = self
                .call_closure_with(closure, from_ref(&value), by_value)?
                .is_truthy();
            match name {
                BuiltinId::Find if matches => return Ok(Value::some(value)),
                BuiltinId::Find => self.discard(iterator, value)?,
                BuiltinId::Position if matches => return Ok(Value::some(Value::Int(index))),
                BuiltinId::Rposition if matches => last_match = Some(index),
                BuiltinId::Any if matches => return Ok(Value::Bool(true)),
                BuiltinId::All if !matches => return Ok(Value::Bool(false)),
                _ => {}
            }
            index += 1;
        }
        Ok(match name {
            BuiltinId::Find | BuiltinId::Position => Value::none(),
            BuiltinId::Rposition => {
                last_match.map_or_else(Value::none, |i| Value::some(Value::Int(i)))
            }
            BuiltinId::Any => Value::Bool(false),
            BuiltinId::All => Value::Bool(true),
            _ => unreachable!(),
        })
    }
}
