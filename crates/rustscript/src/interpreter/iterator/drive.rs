//! Driving an iterator, `next`, `peek`, `zip`, `chain` and the plain methods.

use std::slice::from_ref;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use super::back::supports_back;
use super::in_place;
use super::{
    Handle, IteratorState, Step, chars, int_arg, lines_next, option_inner, value_iter, wrap,
};
use crate::interpreter::bytecode::{BuiltinId, MethodName};
use crate::interpreter::native::Native;
use crate::interpreter::shared::usize_i64;
use crate::interpreter::value::{ClosureData, MapKind, Value};
use crate::interpreter::vm::Vm;

impl Vm {
    /// the receiver mutates in place through its `&mut self`
    pub(super) fn call_user_next(self: &Arc<Self>, value: &Value) -> Result<Value> {
        let Some(chunk) = self
            .impls
            .of_value(value)
            .and_then(|methods| methods.next.clone())
        else {
            bail!("{} is not an iterator", value.type_name());
        };
        self.run_chunk(&chunk, from_ref(value), &[])
    }

    pub(in crate::interpreter) fn has_user_next(&self, value: &Value) -> bool {
        self.impls
            .of_value(value)
            .is_some_and(|methods| methods.next.is_some())
    }

    /// The iterator of a `for` loop. An owned vec hands its items over, nothing else holds them.
    pub(in crate::interpreter) fn loop_iterator(
        self: &Arc<Self>,
        value: Value,
        owned: bool,
    ) -> Result<Value> {
        if owned
            && let Value::Vec(values) = &value
            && !self.has_user_next(&value)
        {
            let items = std::mem::take(&mut *values.lock());
            return Ok(wrap(IteratorState::Owned {
                values: items,
                index: 0,
                vec: true,
            }));
        }
        self.iterator_value(value)
    }

    pub(in crate::interpreter) fn iterator_value(self: &Arc<Self>, value: Value) -> Result<Value> {
        if self.has_user_next(&value) {
            return Ok(wrap(IteratorState::UserNext { value }));
        }
        Ok(match value {
            Value::Native(native)
                if matches!(&*native.lock(), Native::Iterator(_) | Native::Lines(_)) =>
            {
                Value::Native(native)
            }
            Value::Vec(values) | Value::Tuple(values) => value_iter(values),
            Value::Map(map, kind) => {
                let map = map.lock();
                let owned = match kind {
                    MapKind::Map => map
                        .iter()
                        .map(|(k, v)| Value::tuple(vec![k.to_value(), v.clone()]))
                        .collect(),
                    MapKind::Set => map
                        .keys()
                        .map(crate::interpreter::value::MapKey::to_value)
                        .collect(),
                };
                wrap(IteratorState::Owned {
                    values: owned,
                    index: 0,
                    vec: false,
                })
            }
            Value::Range {
                start,
                end,
                inclusive,
            } => wrap(IteratorState::Range {
                next: start,
                end,
                inclusive,
            }),
            Value::Str(source) => chars(source),
            // `Option` yields its payload once, `Result` yields an `Ok` once
            ref wrapped @ Value::Enum { ref def, .. }
                if matches!(
                    def.kind,
                    crate::interpreter::enum_def::EnumKind::Option
                        | crate::interpreter::enum_def::EnumKind::Result
                ) =>
            {
                let yields = wrapped.success_payload().into_iter().collect();
                wrap(IteratorState::Owned {
                    values: yields,
                    index: 0,
                    vec: false,
                })
            }
            other => bail!("{} is not iterable", other.type_name()),
        })
    }

    pub(super) fn skip_then_next(
        self: &Arc<Self>,
        source: &Handle,
        count: usize,
    ) -> Result<Option<Value>> {
        for _ in 0..count {
            if self.iterator_next(source)?.is_none() {
                return Ok(None);
            }
        }
        self.iterator_next(source)
    }

    pub(super) fn zip_next(
        self: &Arc<Self>,
        left: &Handle,
        right: &Handle,
    ) -> Result<Option<Value>> {
        let Some(first) = self.iterator_next(left)? else {
            return Ok(None);
        };
        let Some(second) = self.iterator_next(right)? else {
            return Ok(None);
        };
        Ok(Some(Value::tuple(vec![first, second])))
    }

    pub(super) fn chain_next(
        self: &Arc<Self>,
        iterator: &Handle,
        left: &Handle,
        right: &Handle,
        left_done: bool,
    ) -> Result<Option<Value>> {
        if !left_done {
            if let Some(value) = self.iterator_next(left)? {
                return Ok(Some(value));
            }
            if let Native::Iterator(IteratorState::Chain { left_done, .. }) = &mut *iterator.lock()
            {
                *left_done = true;
            }
        }
        self.iterator_next(right)
    }

    pub(in crate::interpreter) fn iterator_next(
        self: &Arc<Self>,
        iterator: &Handle,
    ) -> Result<Option<Value>> {
        if matches!(&*iterator.lock(), Native::Lines(_)) {
            return Ok(lines_next(iterator));
        }
        let step = {
            let mut native = iterator.lock();
            let Native::Iterator(state) = &mut *native else {
                bail!("{} is not an iterator", native.type_name());
            };
            state.step()
        };
        match step {
            Step::Ready(value) => Ok(value),
            Step::User(value) => {
                let out = self.call_user_next(&value)?;
                Ok(out.some_payload())
            }
            Step::Map(source, closure) => match self.iterator_next(&source)? {
                Some(value) => Ok(Some(self.call_closure_data(&closure, &[value])?)),
                None => Ok(None),
            },
            Step::Filter(source, closure) => loop {
                let Some(value) = self.iterator_next(&source)? else {
                    return Ok(None);
                };
                if self
                    .call_closure_data(&closure, from_ref(&value))?
                    .is_truthy()
                {
                    return Ok(Some(value));
                }
            },
            Step::FilterMap(source, closure) => loop {
                let Some(value) = self.iterator_next(&source)? else {
                    return Ok(None);
                };
                if let Some(inner) = option_inner(&self.call_closure_data(&closure, &[value])?) {
                    return Ok(Some(inner));
                }
            },
            Step::Enumerate(source, index) => Ok(self
                .iterator_next(&source)?
                .map(|value| Value::tuple(vec![Value::Int(usize_i64(index)), value]))),
            Step::Zip(left, right) => self.zip_next(&left, &right),
            Step::Chain(left, right, left_done) => {
                self.chain_next(iterator, &left, &right, left_done)
            }
            Step::Take(source) => self.iterator_next(&source),
            Step::Rev(source) => self.iterator_next_back(&source),
            Step::Cloned(source) => Ok(self.iterator_next(&source)?.map(|v| v.deep_clone())),
            Step::Stride(source, count) | Step::Skip(source, count) => {
                self.skip_then_next(&source, count)
            }
            Step::TakeWhile(source, closure) => {
                let Some(value) = self.iterator_next(&source)? else {
                    return Ok(None);
                };
                if self
                    .call_closure_data(&closure, from_ref(&value))?
                    .is_truthy()
                {
                    Ok(Some(value))
                } else {
                    if let Native::Iterator(IteratorState::TakeWhile { done, .. }) =
                        &mut *iterator.lock()
                    {
                        *done = true;
                    }
                    Ok(None)
                }
            }
            Step::SkipWhile(source, closure, skipping) => {
                let mut still_skipping = skipping;
                loop {
                    let Some(value) = self.iterator_next(&source)? else {
                        return Ok(None);
                    };
                    if !still_skipping
                        || !self
                            .call_closure_data(&closure, from_ref(&value))?
                            .is_truthy()
                    {
                        if still_skipping
                            && let Native::Iterator(IteratorState::SkipWhile { skipping, .. }) =
                                &mut *iterator.lock()
                        {
                            *skipping = false;
                        }
                        return Ok(Some(value));
                    }
                    still_skipping = true;
                }
            }
        }
    }

    pub(in crate::interpreter) fn call_closure_data(
        self: &Arc<Self>,
        clo: &Arc<ClosureData>,
        args: &[Value],
    ) -> Result<Value> {
        self.run_chunk(&clo.chunk, args, &clo.captured)
    }

    pub(in crate::interpreter) fn drain_items(
        self: &Arc<Self>,
        value: Value,
    ) -> Result<Vec<Value>> {
        let Value::Native(iterator) = self.iterator_value(value)? else {
            unreachable!();
        };
        let mut items = Vec::new();
        while let Some(item) = self.iterator_next(&iterator)? {
            items.push(item);
        }
        Ok(items)
    }

    /// `None` when the handle is not a peekable.
    pub(super) fn peek(self: &Arc<Self>, iterator: &Handle) -> Result<Option<Value>> {
        let (buffered, source) = match &*iterator.lock() {
            Native::Iterator(IteratorState::Peekable { buffered, source }) => {
                (buffered.clone(), source.clone())
            }
            _ => return Ok(None),
        };
        if let Some(item) = buffered {
            return Ok(Some(Value::some(item)));
        }
        let item = self.iterator_next(&source)?;
        if let Native::Iterator(IteratorState::Peekable { buffered, .. }) = &mut *iterator.lock() {
            buffered.clone_from(&item);
        }
        Ok(Some(match item {
            Some(item) => Value::some(item),
            None => Value::none(),
        }))
    }

    pub(super) fn iterator_count(self: &Arc<Self>, iterator: &Handle) -> Result<Value> {
        let mut count: usize = 0;
        while self.iterator_next(iterator)?.is_some() {
            count += 1;
        }
        Ok(crate::interpreter::shared::usize_value(count))
    }

    pub(super) fn iterator_last(self: &Arc<Self>, iterator: &Handle) -> Result<Value> {
        let mut last = None;
        while let Some(item) = self.iterator_next(iterator)? {
            last = Some(item);
        }
        Ok(last.map_or_else(Value::none, Value::some))
    }

    /// `b` is any value that iterates
    pub(super) fn zip_or_chain(
        self: &Arc<Self>,
        iterator: &Handle,
        method: &MethodName,
        args: &[Value],
    ) -> Result<Value> {
        let other = args
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("{} takes an iterator", method.text))?;
        let Value::Native(right) = self.iterator_value(other)? else {
            bail!("{} takes an iterator", method.text);
        };
        Ok(if method.id == BuiltinId::Zip {
            wrap(IteratorState::Zip {
                left: iterator.clone(),
                right,
            })
        } else {
            wrap(IteratorState::Chain {
                left: iterator.clone(),
                right,
                left_done: false,
            })
        })
    }

    pub(in crate::interpreter) fn iterator_method(
        self: &Arc<Self>,
        iterator: &Handle,
        method: &MethodName,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let scalar = method.scalar.as_ref();
        let value = match method.id {
            BuiltinId::Enumerate => wrap(IteratorState::Enumerate {
                source: iterator.clone(),
                index: 0,
            }),
            BuiltinId::Zip | BuiltinId::Chain => self.zip_or_chain(iterator, method, args)?,
            BuiltinId::Take => wrap(IteratorState::Take {
                source: iterator.clone(),
                remaining: usize::try_from(int_arg(args)?)?,
            }),
            BuiltinId::Skip => wrap(IteratorState::Skip {
                source: iterator.clone(),
                remaining: usize::try_from(int_arg(args)?)?,
            }),
            BuiltinId::StepBy => {
                let step = usize::try_from(int_arg(args)?)?;
                if step == 0 {
                    bail!("assertion failed: step != 0");
                }
                wrap(IteratorState::StepBy {
                    source: iterator.clone(),
                    step,
                    first: true,
                })
            }
            BuiltinId::Peekable => wrap(IteratorState::Peekable {
                source: iterator.clone(),
                buffered: None,
            }),
            BuiltinId::Cloned | BuiltinId::Copied => wrap(IteratorState::Cloned {
                source: iterator.clone(),
            }),
            // iterators are shared handles, so handing the same one back is the `by_ref` borrow
            BuiltinId::ByRef => Value::Native(iterator.clone()),
            BuiltinId::Next => self
                .iterator_next(iterator)?
                .map_or_else(Value::none, Value::some),
            BuiltinId::Nth => {
                let index = usize::try_from(int_arg(args)?)?;
                let mut item = None;
                for _ in 0..=index {
                    item = self.iterator_next(iterator)?;
                    if item.is_none() {
                        break;
                    }
                }
                item.map_or_else(Value::none, Value::some)
            }
            BuiltinId::Peek => match self.peek(iterator)? {
                Some(v) => v,
                None => return Ok(None),
            },
            BuiltinId::Count => self.iterator_count(iterator)?,
            // every iterator is driven forwards, so `last` drains to the end
            BuiltinId::NextBack if supports_back(iterator) => self
                .iterator_next_back(iterator)?
                .map_or_else(Value::none, Value::some),
            BuiltinId::Last | BuiltinId::NextBack => self.iterator_last(iterator)?,
            BuiltinId::Sum => self.iterator_sum(iterator, scalar)?,
            BuiltinId::Product => self.iterator_product(iterator, scalar)?,
            BuiltinId::Max | BuiltinId::Min => self.iterator_extreme(iterator, method.id)?,
            BuiltinId::Collect | BuiltinId::ToVec => {
                if in_place::collects_nothing(iterator, scalar) {
                    Value::vec(Vec::new())
                } else {
                    Value::vec(self.drain_iterator(iterator)?)
                }
            }
            BuiltinId::CollectString => Value::str(
                self.drain_iterator(iterator)?
                    .iter()
                    .map(Value::display)
                    .collect::<String>(),
            ),
            BuiltinId::CollectMap => {
                crate::interpreter::vecmap::collect_map(self.drain_iterator(iterator)?)?
            }
            BuiltinId::CollectSet => {
                crate::interpreter::vecmap::collect_set(self.drain_iterator(iterator)?)?
            }
            BuiltinId::Rev if supports_back(iterator) => wrap(IteratorState::Rev {
                source: iterator.clone(),
            }),
            BuiltinId::Rev => {
                let mut items = self.drain_iterator(iterator)?;
                items.reverse();
                Value::vec(items)
            }
            // `Chars::as_str` is the unconsumed tail, the capitalize idiom. Only a char iterator
            // still knows its source.
            BuiltinId::AsStr => match &*iterator.lock() {
                Native::Iterator(IteratorState::Chars { source, offset }) => {
                    Value::str(source[*offset..].to_string())
                }
                _ => return Ok(None),
            },
            _ => return Ok(None),
        };
        Ok(Some(value))
    }
}
