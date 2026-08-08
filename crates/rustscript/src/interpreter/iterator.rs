use num_traits::AsPrimitive;
use std::cell::RefCell;
use std::rc::Rc;
use std::slice::from_ref;

use anyhow::{Result, anyhow, bail};

use super::Interp;
use super::builtins::{as_closure, option_inner};
use super::bytecode::{BuiltinId, MethodName, ScalarTy};
use super::native::{Native, lines_next};
use super::ops::compare_values;
use super::regex_bridge::{CapturesValue, MatchValue, RegexValue};
use super::shared::usize_i64;
use super::value::{ClosureData, Map, MapKey, MapKind, RStr, Value, ValueRef};

type Handle = Rc<RefCell<Native>>;

pub enum IteratorState {
    Values {
        values: Rc<RefCell<Vec<Value>>>,
        index: usize,
    },
    MutableValues {
        values: Rc<RefCell<Vec<Value>>>,
        index: usize,
    },
    /// A borrow of an eager vector taken with `by_ref`. It takes from the front
    /// of the shared vector rather than walking an index of its own, so
    /// whatever it hands out is gone from the borrowed iterator too. That is
    /// the whole point of `by_ref`, and a cursor could not do it, since the
    /// cursor would die with the borrow and leave the source untouched.
    DrainingValues {
        values: Rc<RefCell<Vec<Value>>>,
    },
    Owned {
        values: Vec<Value>,
        index: usize,
    },
    Range {
        next: i64,
        end: i64,
        inclusive: bool,
    },
    Bytes {
        source: Rc<RStr>,
        index: usize,
    },
    Chars {
        source: Rc<RStr>,
        offset: usize,
    },
    Lines {
        source: Rc<RStr>,
        offset: usize,
    },
    SplitWhitespace {
        source: Rc<RStr>,
        offset: usize,
    },
    RegexFind {
        regex: RegexValue,
        source: Rc<RStr>,
        offset: usize,
    },
    RegexCaptures {
        regex: RegexValue,
        source: Rc<RStr>,
        offset: usize,
    },
    Map {
        source: Handle,
        closure: Rc<ClosureData>,
    },
    Filter {
        source: Handle,
        closure: Rc<ClosureData>,
    },
    FilterMap {
        source: Handle,
        closure: Rc<ClosureData>,
    },
    Enumerate {
        source: Handle,
        index: usize,
    },
    Take {
        source: Handle,
        remaining: usize,
    },
    Skip {
        source: Handle,
        remaining: usize,
    },
    TakeWhile {
        source: Handle,
        closure: Rc<ClosureData>,
        done: bool,
    },
    SkipWhile {
        source: Handle,
        closure: Rc<ClosureData>,
        skipping: bool,
    },
    /// `peekable`. Holds at most one item pulled early by `peek`, which the
    /// next `next` hands back before touching the source again.
    Peekable {
        source: Handle,
        buffered: Option<Value>,
    },
}

enum Step {
    Ready(Option<Value>),
    Map(Handle, Rc<ClosureData>),
    Filter(Handle, Rc<ClosureData>),
    FilterMap(Handle, Rc<ClosureData>),
    Enumerate(Handle, usize),
    Take(Handle),
    Skip(Handle, usize),
    TakeWhile(Handle, Rc<ClosureData>),
    SkipWhile(Handle, Rc<ClosureData>, bool),
}

pub(super) fn wrap(state: IteratorState) -> Value {
    Native::Iterator(state).wrap()
}

pub(super) fn value_iter(items: Rc<RefCell<Vec<Value>>>) -> Value {
    wrap(IteratorState::Values {
        values: items,
        index: 0,
    })
}

pub(super) fn value_iter_mut(items: Rc<RefCell<Vec<Value>>>) -> Value {
    wrap(IteratorState::MutableValues {
        values: items,
        index: 0,
    })
}

pub(super) fn draining_iter(items: Rc<RefCell<Vec<Value>>>) -> Value {
    wrap(IteratorState::DrainingValues { values: items })
}

pub(super) fn bytes(source: Rc<RStr>) -> Value {
    wrap(IteratorState::Bytes { source, index: 0 })
}

pub(super) fn chars(source: Rc<RStr>) -> Value {
    wrap(IteratorState::Chars { source, offset: 0 })
}

pub(super) fn lines(source: Rc<RStr>) -> Value {
    wrap(IteratorState::Lines { source, offset: 0 })
}

pub(super) fn split_whitespace(source: Rc<RStr>) -> Value {
    wrap(IteratorState::SplitWhitespace { source, offset: 0 })
}

pub(super) fn regex_find(regex: RegexValue, source: Rc<RStr>) -> Value {
    wrap(IteratorState::RegexFind {
        regex,
        source,
        offset: 0,
    })
}

pub(super) fn regex_captures(regex: RegexValue, source: Rc<RStr>) -> Value {
    wrap(IteratorState::RegexCaptures {
        regex,
        source,
        offset: 0,
    })
}

fn next_line(source: &RStr, offset: &mut usize) -> Option<Value> {
    if *offset >= source.len() {
        return None;
    }
    let rest = &source[*offset..];
    let line = rest.lines().next()?;
    let mut consumed = line.len();
    if rest[consumed..].starts_with("\r\n") {
        consumed += 2;
    } else if rest[consumed..].starts_with('\n') {
        consumed += 1;
    }
    *offset += consumed;
    Some(Value::str(line))
}

fn next_word(source: &RStr, offset: &mut usize) -> Option<Value> {
    let rest = &source[*offset..];
    let word = rest.split_whitespace().next()?;
    let start = word.as_ptr() as usize - rest.as_ptr() as usize;
    *offset += start + word.len();
    Some(Value::str(word))
}

fn next_regex_offset(source: &str, start: usize, end: usize) -> usize {
    if end > start {
        return end;
    }
    if end == source.len() {
        return source.len() + 1;
    }
    end + source[end..].chars().next().map_or(1, char::len_utf8)
}

impl IteratorState {
    fn step(&mut self) -> Step {
        match self {
            IteratorState::Values { values, index } => {
                let value = values.borrow().get(*index).cloned();
                *index += usize::from(value.is_some());
                Step::Ready(value)
            }
            IteratorState::MutableValues { values, index } => {
                let exists = *index < values.borrow().len();
                let value = exists
                    .then(|| Value::Ref(Rc::new(ValueRef::vec_element(values.clone(), *index))));
                *index += usize::from(exists);
                Step::Ready(value)
            }
            IteratorState::DrainingValues { values } => {
                let mut items = values.borrow_mut();
                let value = if items.is_empty() {
                    None
                } else {
                    Some(items.remove(0))
                };
                Step::Ready(value)
            }
            IteratorState::Owned { values, index } => {
                let value = values.get(*index).cloned();
                *index += usize::from(value.is_some());
                Step::Ready(value)
            }
            IteratorState::Range {
                next,
                end,
                inclusive,
            } => range_step(next, *end, *inclusive),
            IteratorState::Bytes { source, index } => bytes_step(source, index),
            IteratorState::Chars { source, offset } => chars_step(source, offset),
            IteratorState::Lines { source, offset } => Step::Ready(next_line(source, offset)),
            IteratorState::SplitWhitespace { source, offset } => {
                Step::Ready(next_word(source, offset))
            }
            IteratorState::RegexFind {
                regex,
                source,
                offset,
            } => regex_find_step(regex, source, offset),
            IteratorState::RegexCaptures {
                regex,
                source,
                offset,
            } => regex_captures_step(regex, source, offset),
            IteratorState::Map { source, closure } => Step::Map(source.clone(), closure.clone()),
            IteratorState::Filter { source, closure } => {
                Step::Filter(source.clone(), closure.clone())
            }
            IteratorState::FilterMap { source, closure } => {
                Step::FilterMap(source.clone(), closure.clone())
            }
            IteratorState::Enumerate { source, index } => {
                let current = *index;
                *index += 1;
                Step::Enumerate(source.clone(), current)
            }
            IteratorState::Take { source, remaining } => {
                if *remaining == 0 {
                    Step::Ready(None)
                } else {
                    *remaining -= 1;
                    Step::Take(source.clone())
                }
            }
            IteratorState::Skip { source, remaining } => {
                let count = *remaining;
                *remaining = 0;
                Step::Skip(source.clone(), count)
            }
            IteratorState::TakeWhile {
                source,
                closure,
                done,
            } => {
                if *done {
                    Step::Ready(None)
                } else {
                    Step::TakeWhile(source.clone(), closure.clone())
                }
            }
            IteratorState::SkipWhile {
                source,
                closure,
                skipping,
            } => Step::SkipWhile(source.clone(), closure.clone(), *skipping),
            IteratorState::Peekable { source, buffered } => match buffered.take() {
                Some(item) => Step::Ready(Some(item)),
                None => Step::Take(source.clone()),
            },
        }
    }
}

/// One step over a string's bytes.
fn bytes_step(source: &RStr, index: &mut usize) -> Step {
    let value = source.as_bytes().get(*index).copied();
    *index += usize::from(value.is_some());
    Step::Ready(value.map(|byte| Value::Int(i64::from(byte))))
}

/// One step over a string's chars, advancing by the char's own width.
fn chars_step(source: &RStr, offset: &mut usize) -> Step {
    let value = source[*offset..].chars().next();
    if let Some(ch) = value {
        *offset += ch.len_utf8();
    }
    Step::Ready(value.map(Value::Char))
}

/// One step of an integer range, exclusive or inclusive.
fn range_step(next: &mut i64, end: i64, inclusive: bool) -> Step {
    let done = if inclusive { *next > end } else { *next >= end };
    if done {
        Step::Ready(None)
    } else {
        let value = *next;
        *next += 1;
        Step::Ready(Some(Value::Int(value)))
    }
}

/// One `find_iter` step, advancing past the match or an empty match's char.
fn regex_find_step(regex: &RegexValue, source: &Rc<RStr>, offset: &mut usize) -> Step {
    if *offset > source.len() {
        return Step::Ready(None);
    }
    let Some(found) = regex.compiled.find_at(source, *offset) else {
        return Step::Ready(None);
    };
    *offset = next_regex_offset(source, found.start(), found.end());
    Step::Ready(Some(
        Native::RegexMatch(MatchValue {
            source: source.clone(),
            start: found.start(),
            end: found.end(),
        })
        .wrap(),
    ))
}

/// One `captures_iter` step, the groups as spans over the same source.
fn regex_captures_step(regex: &RegexValue, source: &Rc<RStr>, offset: &mut usize) -> Step {
    if *offset > source.len() {
        return Step::Ready(None);
    }
    let Some(captures) = regex.compiled.captures_at(source, *offset) else {
        return Step::Ready(None);
    };
    let Some(found) = captures.get(0) else {
        return Step::Ready(None);
    };
    *offset = next_regex_offset(source, found.start(), found.end());
    let groups = (0..captures.len())
        .map(|index| captures.get(index).map(|m| (m.start(), m.end())))
        .collect();
    Step::Ready(Some(
        Native::RegexCaptures(CapturesValue {
            source: source.clone(),
            groups,
            names: regex.names.clone(),
        })
        .wrap(),
    ))
}

impl Interp {
    pub(super) fn iterator_value(value: Value) -> Result<Value> {
        Ok(match value {
            Value::Native(native)
                if matches!(&*native.borrow(), Native::Iterator(_) | Native::Lines(_)) =>
            {
                Value::Native(native)
            }
            Value::Vec(values) | Value::Tuple(values) => value_iter(values),
            Value::Map(map, kind) => {
                let map = map.borrow();
                // A set iterates its elements, a map its (key, value) pairs.
                let owned = match kind {
                    MapKind::Map => map_items(&map),
                    MapKind::Set => map.keys().map(MapKey::to_value).collect(),
                };
                wrap(IteratorState::Owned {
                    values: owned,
                    index: 0,
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
            other => bail!("{} is not iterable", other.type_name()),
        })
    }

    pub(super) fn iterator_next(&self, iterator: &Handle) -> Result<Option<Value>> {
        if matches!(&*iterator.borrow(), Native::Lines(_)) {
            return Ok(lines_next(iterator));
        }
        let step = {
            let mut native = iterator.borrow_mut();
            let Native::Iterator(state) = &mut *native else {
                bail!("{} is not an iterator", native.type_name());
            };
            state.step()
        };
        match step {
            Step::Ready(value) => Ok(value),
            Step::Map(source, closure) => match self.iterator_next(&source)? {
                Some(value) => Ok(Some(self.call_closure(&closure, &[value])?)),
                None => Ok(None),
            },
            Step::Filter(source, closure) => loop {
                let Some(value) = self.iterator_next(&source)? else {
                    return Ok(None);
                };
                if self.call_closure(&closure, from_ref(&value))?.is_truthy() {
                    return Ok(Some(value));
                }
            },
            Step::FilterMap(source, closure) => loop {
                let Some(value) = self.iterator_next(&source)? else {
                    return Ok(None);
                };
                if let Some(inner) = option_inner(&self.call_closure(&closure, &[value])?) {
                    return Ok(Some(inner));
                }
            },
            Step::Enumerate(source, index) => Ok(self.iterator_next(&source)?.map(|value| {
                Value::Tuple(Rc::new(RefCell::new(vec![
                    Value::Int(usize_i64(index)),
                    value,
                ])))
            })),
            Step::Take(source) => self.iterator_next(&source),
            Step::Skip(source, count) => {
                for _ in 0..count {
                    if self.iterator_next(&source)?.is_none() {
                        return Ok(None);
                    }
                }
                self.iterator_next(&source)
            }
            Step::TakeWhile(source, closure) => {
                let Some(value) = self.iterator_next(&source)? else {
                    return Ok(None);
                };
                if self.call_closure(&closure, from_ref(&value))?.is_truthy() {
                    Ok(Some(value))
                } else {
                    if let Native::Iterator(IteratorState::TakeWhile { done, .. }) =
                        &mut *iterator.borrow_mut()
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
                        || !self.call_closure(&closure, from_ref(&value))?.is_truthy()
                    {
                        if still_skipping
                            && let Native::Iterator(IteratorState::SkipWhile { skipping, .. }) =
                                &mut *iterator.borrow_mut()
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

    pub(super) fn iter_items(&self, value: Value) -> Result<Vec<Value>> {
        let Value::Native(iterator) = Self::iterator_value(value)? else {
            unreachable!();
        };
        let mut items = Vec::new();
        while let Some(item) = self.iterator_next(&iterator)? {
            items.push(item);
        }
        Ok(items)
    }

    pub(super) fn iterator_method(
        &self,
        iterator: &Handle,
        method: &MethodName,
        args: &[Value],
    ) -> Result<Option<Value>> {
        use BuiltinId as B;
        let value = match method.id {
            B::Enumerate => wrap(IteratorState::Enumerate {
                source: iterator.clone(),
                index: 0,
            }),
            B::Take => wrap(IteratorState::Take {
                source: iterator.clone(),
                remaining: usize::try_from(int_arg(args)?)?,
            }),
            B::Skip => wrap(IteratorState::Skip {
                source: iterator.clone(),
                remaining: usize::try_from(int_arg(args)?)?,
            }),
            B::Count => {
                let mut count = 0;
                while self.iterator_next(iterator)?.is_some() {
                    count += 1;
                }
                Value::Int(count)
            }
            B::Sum => self.iterator_sum(iterator, method.scalar.as_ref())?,
            B::Product => self.iterator_product(iterator)?,
            _ => match method.text.as_str() {
                "next" => self
                    .iterator_next(iterator)?
                    .map_or_else(Value::none, Value::some),
                "last" => {
                    let mut last = None;
                    while let Some(item) = self.iterator_next(iterator)? {
                        last = Some(item);
                    }
                    last.map_or_else(Value::none, Value::some)
                }
                "collect" | "to_vec" => Value::vec(self.drain_iterator(iterator)?),
                "collect_string" => Value::str(
                    self.drain_iterator(iterator)?
                        .iter()
                        .map(Value::display)
                        .collect::<String>(),
                ),
                // `by_ref` borrows the iterator so the caller keeps it after
                // the adaptor is done with it. Iterators are shared handles
                // here, so handing the same one back is that borrow.
                "cloned" | "copied" | "by_ref" => Value::Native(iterator.clone()),
                "peekable" => wrap(IteratorState::Peekable {
                    source: iterator.clone(),
                    buffered: None,
                }),
                // `peek` pulls one item early and keeps it, so the value is
                // still there for the next `next`.
                "peek" => {
                    let buffered = match &*iterator.borrow() {
                        Native::Iterator(IteratorState::Peekable { buffered, .. }) => {
                            buffered.clone()
                        }
                        _ => return Ok(None),
                    };
                    if let Some(item) = buffered {
                        return Ok(Some(Value::some(item)));
                    }
                    let source = match &*iterator.borrow() {
                        Native::Iterator(IteratorState::Peekable { source, .. }) => source.clone(),
                        _ => return Ok(None),
                    };
                    let item = self.iterator_next(&source)?;
                    if let Native::Iterator(IteratorState::Peekable { buffered, .. }) =
                        &mut *iterator.borrow_mut()
                    {
                        buffered.clone_from(&item);
                    }
                    match item {
                        Some(item) => Value::some(item),
                        None => Value::none(),
                    }
                }
                "rev" => {
                    let mut items = self.drain_iterator(iterator)?;
                    items.reverse();
                    Value::vec(items)
                }
                "max" | "min" => self.iterator_extreme(iterator, method.text.as_str())?,
                // `Chars::as_str` gives the not yet consumed tail, which is what
                // makes the `chars.next()` then `chars.as_str()` capitalize idiom
                // work. Only a char iterator still knows its source text.
                "as_str" => match &*iterator.borrow() {
                    Native::Iterator(IteratorState::Chars { source, offset }) => {
                        Value::str(source[*offset..].to_string())
                    }
                    _ => return Ok(None),
                },
                _ => return Ok(None),
            },
        };
        Ok(Some(value))
    }

    pub(super) fn iterator_higher_order(
        &self,
        iterator: &Handle,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let closure = |index| as_closure(args.get(index));
        let value = match name {
            "map" => wrap(IteratorState::Map {
                source: iterator.clone(),
                closure: closure(0)?,
            }),
            "filter" => wrap(IteratorState::Filter {
                source: iterator.clone(),
                closure: closure(0)?,
            }),
            "filter_map" => wrap(IteratorState::FilterMap {
                source: iterator.clone(),
                closure: closure(0)?,
            }),
            "take_while" => wrap(IteratorState::TakeWhile {
                source: iterator.clone(),
                closure: closure(0)?,
                done: false,
            }),
            "skip_while" => wrap(IteratorState::SkipWhile {
                source: iterator.clone(),
                closure: closure(0)?,
                skipping: true,
            }),
            "for_each" => {
                let closure = closure(0)?;
                while let Some(value) = self.iterator_next(iterator)? {
                    self.call_closure(&closure, &[value])?;
                }
                Value::Unit
            }
            "find" | "position" | "any" | "all" => {
                return self
                    .iterator_predicate(iterator, name, &closure(0)?)
                    .map(Some);
            }
            "fold" => {
                let closure = closure(1)?;
                let mut accumulator = args.first().cloned().unwrap_or(Value::Unit);
                while let Some(value) = self.iterator_next(iterator)? {
                    accumulator = self.call_closure(&closure, &[accumulator, value])?;
                }
                accumulator
            }
            "reduce" => {
                let closure = closure(0)?;
                let Some(mut accumulator) = self.iterator_next(iterator)? else {
                    return Ok(Some(Value::none()));
                };
                while let Some(value) = self.iterator_next(iterator)? {
                    accumulator = self.call_closure(&closure, &[accumulator, value])?;
                }
                Value::some(accumulator)
            }
            "flat_map" => {
                let closure = closure(0)?;
                let mut output = Vec::new();
                while let Some(value) = self.iterator_next(iterator)? {
                    let mapped = self.call_closure(&closure, &[value])?;
                    output.extend(self.iter_items(mapped)?);
                }
                Value::vec(output)
            }
            "partition" => {
                let closure = closure(0)?;
                let (mut yes, mut no) = (Vec::new(), Vec::new());
                while let Some(value) = self.iterator_next(iterator)? {
                    if self.call_closure(&closure, from_ref(&value))?.is_truthy() {
                        yes.push(value);
                    } else {
                        no.push(value);
                    }
                }
                Value::Tuple(Rc::new(RefCell::new(vec![Value::vec(yes), Value::vec(no)])))
            }
            "max_by_key" | "min_by_key" => {
                let closure = closure(0)?;
                let mut best: Option<(Value, Value)> = None;
                while let Some(value) = self.iterator_next(iterator)? {
                    let key = self.call_closure(&closure, from_ref(&value))?;
                    let take = match &best {
                        None => true,
                        Some((best_key, _)) => {
                            let order = compare_values(&key, best_key)?;
                            if name == "max_by_key" {
                                order.is_ge()
                            } else {
                                order.is_lt()
                            }
                        }
                    };
                    if take {
                        best = Some((key, value));
                    }
                }
                best.map_or_else(Value::none, |(_, value)| Value::some(value))
            }
            _ => return Ok(None),
        };
        Ok(Some(value))
    }

    fn drain_iterator(&self, iterator: &Handle) -> Result<Vec<Value>> {
        let mut values = Vec::new();
        while let Some(value) = self.iterator_next(iterator)? {
            values.push(value);
        }
        Ok(values)
    }

    fn iterator_sum(&self, iterator: &Handle, target: Option<&ScalarTy>) -> Result<Value> {
        // The accumulator is an i128 so a `u64` element past `i64::MAX` keeps
        // its value. Reading elements through `bridge_image` clamped them to
        // `i64::MAX` before they were even added.
        let mut integers = 0i128;
        // `Sum` for floats starts at -0.0, not 0.0, so that summing negative
        // zeros keeps the sign. Starting at 0.0 turned `[-0.0].sum()` into
        // `0.0`, because `0.0 + -0.0` is `0.0`.
        let mut floats = -0.0f64;
        let mut has_float = false;
        // Without a target the sum is a plain i64 and overflows at its bounds,
        // which is what an untyped `sum()` has always done.
        let (low, high) = match target {
            Some(ScalarTy::Int(width)) => (width.min(), width.max()),
            _ => (i128::from(i64::MIN), i128::from(i64::MAX)),
        };
        while let Some(value) = self.iterator_next(iterator)? {
            if let Some((value, _)) = value.int_parts() {
                integers = integers
                    .checked_add(value)
                    .ok_or_else(|| anyhow!("attempt to add with overflow"))?;
                // A `sum::<u8>()` overflows at 255, not at `i64::MAX`, so the
                // target width is what says where the panic belongs.
                if integers < low || integers > high {
                    bail!("attempt to add with overflow");
                }
                continue;
            }
            match value.bridge_image().unwrap_or(value) {
                Value::Float(value) => {
                    floats += value;
                    has_float = true;
                }
                other => bail!("sum needs numbers, got {}", other.type_name()),
            }
        }
        // An empty sequence carries no element to tell an integer sum from a
        // float one, so a `sum::<f64>()` turbofish is the only thing that can.
        // Without it an empty float sum answered `0`, which prints `0.0` after
        // the annotation coerces it, where real Rust prints `-0.0`.
        let float_target = matches!(target, Some(ScalarTy::F32 | ScalarTy::F64));
        Ok(if has_float || (float_target && integers == 0) {
            // Folding the integer side back in would add a +0.0 that cancels
            // the -0.0 identity, so it only joins when it carries a value. A
            // real sum has one element type anyway, the split accumulator is
            // only here because the interpreter cannot see which.
            let total = if integers == 0 {
                floats
            } else {
                floats + AsPrimitive::<f64>::as_(integers)
            };
            if matches!(target, Some(ScalarTy::F32)) {
                Value::F32(AsPrimitive::<f32>::as_(total))
            } else {
                Value::Float(total)
            }
        } else if let Some(ScalarTy::Int(width)) = target {
            // The sum carries the element width, so a later `!` or shift on it
            // computes at that width. An untagged zero made `!0u16` answer -1.
            Value::int_of_width(integers, *width)
        } else {
            Value::Int(i64::try_from(integers).expect("sum is range-checked per step"))
        })
    }

    fn iterator_product(&self, iterator: &Handle) -> Result<Value> {
        let mut integers = 1i64;
        let mut floats = 1f64;
        let mut has_float = false;
        while let Some(value) = self.iterator_next(iterator)? {
            let value = value.bridge_image().unwrap_or(value);
            match value {
                Value::Int(value) => {
                    integers = integers
                        .checked_mul(value)
                        .ok_or_else(|| anyhow!("attempt to multiply with overflow"))?;
                }
                Value::Float(value) => {
                    floats *= value;
                    has_float = true;
                }
                other => bail!("product needs numbers, got {}", other.type_name()),
            }
        }
        Ok(if has_float {
            Value::Float(floats * AsPrimitive::<f64>::as_(integers))
        } else {
            Value::Int(integers)
        })
    }

    fn iterator_extreme(&self, iterator: &Handle, name: &str) -> Result<Value> {
        let mut best: Option<Value> = None;
        while let Some(value) = self.iterator_next(iterator)? {
            let take = match &best {
                None => true,
                Some(current) => {
                    let order = compare_values(&value, current)?;
                    if name == "max" {
                        order.is_gt()
                    } else {
                        order.is_lt()
                    }
                }
            };
            if take {
                best = Some(value);
            }
        }
        Ok(best.map_or_else(Value::none, Value::some))
    }

    fn iterator_predicate(
        &self,
        iterator: &Handle,
        name: &str,
        closure: &Rc<ClosureData>,
    ) -> Result<Value> {
        let mut index = 0;
        while let Some(value) = self.iterator_next(iterator)? {
            let matches = self.call_closure(closure, from_ref(&value))?.is_truthy();
            match name {
                "find" if matches => return Ok(Value::some(value)),
                "position" if matches => return Ok(Value::some(Value::Int(index))),
                "any" if matches => return Ok(Value::Bool(true)),
                "all" if !matches => return Ok(Value::Bool(false)),
                _ => {}
            }
            index += 1;
        }
        Ok(match name {
            "find" | "position" => Value::none(),
            "any" => Value::Bool(false),
            "all" => Value::Bool(true),
            _ => unreachable!(),
        })
    }
}

fn int_arg(args: &[Value]) -> Result<i64> {
    match args.first() {
        Some(Value::Int(value)) if *value >= 0 => Ok(*value),
        _ => bail!("iterator count needs a non-negative integer"),
    }
}

fn map_items(map: &Map) -> Vec<Value> {
    map.iter()
        .map(|(key, value)| {
            Value::Tuple(Rc::new(RefCell::new(vec![key.to_value(), value.clone()])))
        })
        .collect()
}
