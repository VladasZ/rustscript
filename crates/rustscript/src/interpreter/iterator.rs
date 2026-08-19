//! Lazy, stateful iterators.
//! The states live inside a `Native::Iterator`, so an
//! iterator is a shared handle exactly like every other native resource, and
//! `by_ref`, `peekable`, and open-ended ranges keep their real semantics.

use num_traits::AsPrimitive;
use std::slice::from_ref;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use parking_lot::Mutex;

use super::bytecode::{BuiltinId, MethodName, ScalarTy};
use super::native::Native;
use super::ops::compare_values;
use super::regex_bridge::{CapturesValue, MatchValue, RegexValue};
use super::scalar_chain::{ChainReduce, try_reduce};
use super::shared::usize_i64;
use super::value::{ClosureData, List, MapKind, RsStr, Value, ValueRef};
use super::vm::Vm;

type Handle = Arc<Mutex<Native>>;

pub enum IteratorState {
    Values {
        values: List,
        index: usize,
    },
    MutableValues {
        values: List,
        index: usize,
    },
    /// A user struct or enum with its own `Iterator` impl. Each pull calls
    /// its `next` method, which mutates the held value in place through its
    /// `&mut self`.
    UserNext {
        value: Value,
    },
    /// A borrow of an eager vector taken with `by_ref`. It takes from the front
    /// of the shared vector rather than walking an index of its own, so
    /// whatever it hands out is gone from the borrowed iterator too.
    DrainingValues {
        values: List,
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
        source: RsStr,
        index: usize,
    },
    Chars {
        source: RsStr,
        offset: usize,
    },
    Lines {
        source: RsStr,
        offset: usize,
    },
    SplitWhitespace {
        source: RsStr,
        offset: usize,
    },
    RegexFind {
        regex: RegexValue,
        source: RsStr,
        offset: usize,
    },
    RegexCaptures {
        regex: RegexValue,
        source: RsStr,
        offset: usize,
    },
    Map {
        source: Handle,
        closure: Arc<ClosureData>,
    },
    Filter {
        source: Handle,
        closure: Arc<ClosureData>,
    },
    FilterMap {
        source: Handle,
        closure: Arc<ClosureData>,
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
        closure: Arc<ClosureData>,
        done: bool,
    },
    SkipWhile {
        source: Handle,
        closure: Arc<ClosureData>,
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
    /// Pull the next item by calling the user type's own `next` method.
    User(Value),
    Map(Handle, Arc<ClosureData>),
    Filter(Handle, Arc<ClosureData>),
    FilterMap(Handle, Arc<ClosureData>),
    Enumerate(Handle, usize),
    Take(Handle),
    Skip(Handle, usize),
    TakeWhile(Handle, Arc<ClosureData>),
    SkipWhile(Handle, Arc<ClosureData>, bool),
}

pub(super) fn wrap(state: IteratorState) -> Value {
    Native::Iterator(state).wrap()
}

pub(super) fn value_iter(items: List) -> Value {
    wrap(IteratorState::Values {
        values: items,
        index: 0,
    })
}

pub(super) fn value_iter_mut(items: List) -> Value {
    wrap(IteratorState::MutableValues {
        values: items,
        index: 0,
    })
}

pub(super) fn draining_iter(items: List) -> Value {
    wrap(IteratorState::DrainingValues { values: items })
}

pub(super) fn bytes(source: RsStr) -> Value {
    wrap(IteratorState::Bytes { source, index: 0 })
}

pub(super) fn chars(source: RsStr) -> Value {
    wrap(IteratorState::Chars { source, offset: 0 })
}

pub(super) fn lines(source: RsStr) -> Value {
    wrap(IteratorState::Lines { source, offset: 0 })
}

pub(super) fn split_whitespace(source: RsStr) -> Value {
    wrap(IteratorState::SplitWhitespace { source, offset: 0 })
}

pub(super) fn regex_find(regex: RegexValue, source: RsStr) -> Value {
    wrap(IteratorState::RegexFind {
        regex,
        source,
        offset: 0,
    })
}

pub(super) fn regex_captures(regex: RegexValue, source: RsStr) -> Value {
    wrap(IteratorState::RegexCaptures {
        regex,
        source,
        offset: 0,
    })
}

pub(super) fn as_closure(v: Option<&Value>) -> Result<Arc<ClosureData>> {
    match v {
        Some(Value::Closure(c)) => Ok(c.clone()),
        _ => bail!("this method expects a closure argument"),
    }
}

pub(super) fn option_inner(v: &Value) -> Option<Value> {
    match v {
        Value::Enum {
            enum_name,
            variant,
            data,
        } if &**enum_name == "Option" && &**variant == "Some" => {
            Some(data.lock().first().cloned().unwrap_or(Value::Unit))
        }
        _ => None,
    }
}

fn next_line(source: &str, offset: &mut usize) -> Option<Value> {
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

/// The next `split_whitespace` word span at `offset`, advancing past it.
/// Shared by the generic step and the scalar for plan's chunks, so both
/// walk the source identically.
pub(super) fn next_word_span(source: &str, offset: &mut usize) -> Option<(usize, usize)> {
    let rest = &source[*offset..];
    let word = rest.split_whitespace().next()?;
    let start = *offset + (word.as_ptr() as usize - rest.as_ptr() as usize);
    *offset = start + word.len();
    Some((start, *offset))
}

fn next_word(source: &str, offset: &mut usize) -> Option<Value> {
    let (start, end) = next_word_span(source, offset)?;
    Some(Value::str(&source[start..end]))
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

/// What `fast_next` answered: an item or exhaustion produced in place, or
/// a state that needs the full `iterator_next` machinery.
pub(super) enum FastNext {
    Ready(Option<Value>),
    NotSimple,
}

impl IteratorState {
    /// The items a `for` loop produces in place for the simple sources, so a
    /// tight loop skips the handle clone and the `Step` indirection of the
    /// full `iterator_next` machinery.
    pub(super) fn fast_next(&mut self) -> FastNext {
        FastNext::Ready(match self {
            IteratorState::Range {
                next,
                end,
                inclusive,
            } => range_step(next, *end, *inclusive),
            IteratorState::Bytes { source, index } => bytes_step(source, index),
            IteratorState::Chars { source, offset } => chars_step(source, offset),
            _ => return FastNext::NotSimple,
        })
    }

    fn step(&mut self) -> Step {
        match self {
            IteratorState::UserNext { value } => Step::User(value.clone()),
            IteratorState::Values { values, index } => {
                let value = values.lock().get(*index).cloned();
                *index += usize::from(value.is_some());
                Step::Ready(value)
            }
            IteratorState::MutableValues { values, index } => {
                let exists = *index < values.lock().len();
                let value = exists
                    .then(|| Value::Ref(Arc::new(ValueRef::vec_element(values.clone(), *index))));
                *index += usize::from(exists);
                Step::Ready(value)
            }
            IteratorState::DrainingValues { values } => {
                let mut items = values.lock();
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
            } => Step::Ready(range_step(next, *end, *inclusive)),
            IteratorState::Bytes { source, index } => Step::Ready(bytes_step(source, index)),
            IteratorState::Chars { source, offset } => Step::Ready(chars_step(source, offset)),
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
fn bytes_step(source: &str, index: &mut usize) -> Option<Value> {
    let value = source.as_bytes().get(*index).copied();
    *index += usize::from(value.is_some());
    value.map(|byte| Value::Int(i64::from(byte)))
}

/// One step over a string's chars, advancing by the char's own width.
fn chars_step(source: &str, offset: &mut usize) -> Option<Value> {
    let value = source[*offset..].chars().next();
    if let Some(ch) = value {
        *offset += ch.len_utf8();
    }
    value.map(Value::Char)
}

/// One step of an integer range, exclusive or inclusive.
fn range_step(next: &mut i64, end: i64, inclusive: bool) -> Option<Value> {
    let done = if inclusive { *next > end } else { *next >= end };
    if done {
        None
    } else {
        let value = *next;
        *next += 1;
        Some(Value::Int(value))
    }
}

/// The next `find_iter` span at `offset`, advancing past the match or an
/// empty match's char. Shared by the generic step and the scalar for plan's
/// regex chunks, so both walk the source identically.
pub(super) fn regex_find_span(
    regex: &RegexValue,
    source: &str,
    offset: &mut usize,
) -> Option<(usize, usize)> {
    if *offset > source.len() {
        return None;
    }
    let found = regex.compiled.find_at(source, *offset)?;
    *offset = next_regex_offset(source, found.start(), found.end());
    Some((found.start(), found.end()))
}

/// One `find_iter` step over the shared span walk.
fn regex_find_step(regex: &RegexValue, source: &RsStr, offset: &mut usize) -> Step {
    let Some((start, end)) = regex_find_span(regex, source, offset) else {
        return Step::Ready(None);
    };
    Step::Ready(Some(
        Native::RegexMatch(MatchValue {
            source: source.clone(),
            start,
            end,
        })
        .wrap(),
    ))
}

/// One `captures_iter` step, the groups as spans over the same source.
fn regex_captures_step(regex: &RegexValue, source: &RsStr, offset: &mut usize) -> Step {
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

/// The next item of a live line iterator, errors wrapped exactly as the
/// `ForNext` op wraps them.
fn lines_next(handle: &Handle) -> Option<Value> {
    let mut native = handle.lock();
    let Native::Lines(lines) = &mut *native else {
        return None;
    };
    match lines.next() {
        Some(Ok(line)) => Some(Value::ok(Value::str(line))),
        Some(Err(e)) => Some(Value::err(super::native::io_error_value(&e))),
        None => None,
    }
}

impl Vm {
    /// Call the user `next` impl of an iterator value held by `UserNext`.
    /// The receiver mutates in place through its `&mut self`.
    fn call_user_next(self: &Arc<Self>, value: &Value) -> Result<Value> {
        let ty = match value {
            Value::Struct(s) => s.name().to_string(),
            Value::Enum { enum_name, .. } => enum_name.to_string(),
            other => bail!("{} is not an iterator", other.type_name()),
        };
        let Some(chunk) = self.methods.get(&(ty.clone(), "next".to_string())) else {
            bail!("no `next` method on `{ty}`");
        };
        let chunk = chunk.clone();
        self.run_chunk(&chunk, from_ref(value), &[])
    }

    /// Whether the value's user type has its own `Iterator::next`, so a for
    /// loop or an adaptor chain can drive it.
    pub(super) fn has_user_next(&self, value: &Value) -> bool {
        let ty = match value {
            Value::Struct(s) => &**s.name(),
            Value::Enum { enum_name, .. } => &**enum_name,
            _ => return false,
        };
        self.methods
            .contains_key(&(ty.to_string(), "next".to_string()))
    }

    pub(super) fn iterator_value(self: &Arc<Self>, value: Value) -> Result<Value> {
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
                // A set iterates its elements, a map its (key, value) pairs.
                let owned = match kind {
                    MapKind::Map => map
                        .iter()
                        .map(|(k, v)| Value::tuple(vec![k.to_value(), v.clone()]))
                        .collect(),
                    MapKind::Set => map.keys().map(super::value::MapKey::to_value).collect(),
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

    pub(super) fn iterator_next(self: &Arc<Self>, iterator: &Handle) -> Result<Option<Value>> {
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
                Ok(match out {
                    Value::Enum { variant, data, .. } if &*variant == "Some" => {
                        Some(data.lock().first().cloned().unwrap_or(Value::Unit))
                    }
                    _ => None,
                })
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

    /// Invoke a closure held by an iterator state directly.
    pub(super) fn call_closure_data(
        self: &Arc<Self>,
        clo: &Arc<ClosureData>,
        args: &[Value],
    ) -> Result<Value> {
        self.run_chunk(&clo.chunk, args, &clo.captured)
    }

    /// Drain any iterable, lazy iterators included, into a plain vec.
    pub(super) fn drain_items(self: &Arc<Self>, value: Value) -> Result<Vec<Value>> {
        let Value::Native(iterator) = self.iterator_value(value)? else {
            unreachable!();
        };
        let mut items = Vec::new();
        while let Some(item) = self.iterator_next(&iterator)? {
            items.push(item);
        }
        Ok(items)
    }

    pub(super) fn iterator_method(
        self: &Arc<Self>,
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
                if let Some(v) = try_reduce(self, iterator, &ChainReduce::Count)? {
                    v
                } else {
                    let mut count: usize = 0;
                    while self.iterator_next(iterator)?.is_some() {
                        count += 1;
                    }
                    super::shared::usize_value(count)
                }
            }
            B::Sum => {
                match try_reduce(self, iterator, &ChainReduce::Sum(method.scalar.as_ref()))? {
                    Some(v) => v,
                    None => self.iterator_sum(iterator, method.scalar.as_ref())?,
                }
            }
            B::Product => self.iterator_product(iterator, method.scalar.as_ref())?,
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
                "collect_map" => super::vecmap::collect_map(self.drain_iterator(iterator)?)?,
                "collect_set" => super::vecmap::collect_set(self.drain_iterator(iterator)?)?,
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
                    let buffered = match &*iterator.lock() {
                        Native::Iterator(IteratorState::Peekable { buffered, .. }) => {
                            buffered.clone()
                        }
                        _ => return Ok(None),
                    };
                    if let Some(item) = buffered {
                        return Ok(Some(Value::some(item)));
                    }
                    let source = match &*iterator.lock() {
                        Native::Iterator(IteratorState::Peekable { source, .. }) => source.clone(),
                        _ => return Ok(None),
                    };
                    let item = self.iterator_next(&source)?;
                    if let Native::Iterator(IteratorState::Peekable { buffered, .. }) =
                        &mut *iterator.lock()
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
                "as_str" => match &*iterator.lock() {
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
        self: &Arc<Self>,
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
                    self.call_closure_data(&closure, &[value])?;
                }
                Value::Unit
            }
            "find_map" => {
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
            "find" | "position" | "rposition" | "any" | "all" => {
                let closure = closure(0)?;
                let reduce = match name {
                    "any" => Some(ChainReduce::Any(&closure)),
                    "all" => Some(ChainReduce::All(&closure)),
                    _ => None,
                };
                if let Some(reduce) = reduce
                    && let Some(v) = try_reduce(self, iterator, &reduce)?
                {
                    return Ok(Some(v));
                }
                return self.iterator_predicate(iterator, name, &closure).map(Some);
            }
            _ => return self.iterator_reduce_ho(iterator, name, args),
        };
        Ok(Some(value))
    }

    /// The closure reductions that drain the iterator down to one value.
    fn iterator_reduce_ho(
        self: &Arc<Self>,
        iterator: &Handle,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let closure = |index| as_closure(args.get(index));
        let value = match name {
            "fold" => {
                let closure = closure(1)?;
                let mut accumulator = args.first().cloned().unwrap_or(Value::Unit);
                while let Some(value) = self.iterator_next(iterator)? {
                    accumulator = self.call_closure_data(&closure, &[accumulator, value])?;
                }
                accumulator
            }
            "reduce" => {
                let closure = closure(0)?;
                let Some(mut accumulator) = self.iterator_next(iterator)? else {
                    return Ok(Some(Value::none()));
                };
                while let Some(value) = self.iterator_next(iterator)? {
                    accumulator = self.call_closure_data(&closure, &[accumulator, value])?;
                }
                Value::some(accumulator)
            }
            "flat_map" => {
                let closure = closure(0)?;
                let mut output = Vec::new();
                while let Some(value) = self.iterator_next(iterator)? {
                    let mapped = self.call_closure_data(&closure, &[value])?;
                    output.extend(self.drain_items(mapped)?);
                }
                Value::vec(output)
            }
            "partition" => {
                let closure = closure(0)?;
                let (mut yes, mut no) = (Vec::new(), Vec::new());
                while let Some(value) = self.iterator_next(iterator)? {
                    if self
                        .call_closure_data(&closure, from_ref(&value))?
                        .is_truthy()
                    {
                        yes.push(value);
                    } else {
                        no.push(value);
                    }
                }
                Value::tuple(vec![Value::vec(yes), Value::vec(no)])
            }
            "max_by_key" | "min_by_key" => {
                let closure = closure(0)?;
                let mut best: Option<(Value, Value)> = None;
                while let Some(value) = self.iterator_next(iterator)? {
                    let key = self.call_closure_data(&closure, from_ref(&value))?;
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

    fn drain_iterator(self: &Arc<Self>, iterator: &Handle) -> Result<Vec<Value>> {
        let mut values = Vec::new();
        while let Some(value) = self.iterator_next(iterator)? {
            values.push(value);
        }
        Ok(values)
    }

    fn iterator_sum(
        self: &Arc<Self>,
        iterator: &Handle,
        target: Option<&ScalarTy>,
    ) -> Result<Value> {
        let items = self.drain_iterator(iterator)?;
        sum_values(items, target)
    }

    fn iterator_product(
        self: &Arc<Self>,
        iterator: &Handle,
        target: Option<&ScalarTy>,
    ) -> Result<Value> {
        // The accumulator is an i128 for the same reason as `sum_values`,
        // and a `product::<u16>()` overflows at the target width, not at
        // `i64::MAX`, so the turbofish says where the panic belongs.
        let mut integers = 1i128;
        let mut floats = 1f64;
        let mut has_float = false;
        let mut has_int = false;
        let (low, high) = match target {
            Some(ScalarTy::Int(width)) => (width.min(), width.max()),
            _ => (i128::from(i64::MIN), i128::from(i64::MAX)),
        };
        while let Some(value) = self.iterator_next(iterator)? {
            if let Some((value, _)) = value.int_parts() {
                has_int = true;
                integers = integers
                    .checked_mul(value)
                    .ok_or_else(|| anyhow!("attempt to multiply with overflow"))?;
                if integers < low || integers > high {
                    bail!("attempt to multiply with overflow");
                }
                continue;
            }
            match value.bridge_image().unwrap_or(value) {
                Value::Float(value) => {
                    floats *= value;
                    has_float = true;
                }
                other => bail!("product needs numbers, got {}", other.type_name()),
            }
        }
        // An empty sequence carries no element to tell an integer product
        // from a float one, so a `product::<f64>()` turbofish is the only
        // thing that can.
        let float_target = matches!(target, Some(ScalarTy::F32 | ScalarTy::F64));
        Ok(if has_float || (float_target && !has_int) {
            let total = floats * AsPrimitive::<f64>::as_(integers);
            if matches!(target, Some(ScalarTy::F32)) {
                Value::F32(AsPrimitive::<f32>::as_(total))
            } else {
                Value::Float(total)
            }
        } else if let Some(ScalarTy::Int(width)) = target {
            // The product carries the target width, so a later `checked_*`
            // on it computes at that width. An untagged result made
            // `product::<u16>().checked_mul(..)` miss its overflow.
            Value::int_of_width(integers, *width)
        } else {
            Value::Int(i64::try_from(integers).expect("product is range-checked per step"))
        })
    }

    fn iterator_extreme(self: &Arc<Self>, iterator: &Handle, name: &str) -> Result<Value> {
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
        self: &Arc<Self>,
        iterator: &Handle,
        name: &str,
        closure: &Arc<ClosureData>,
    ) -> Result<Value> {
        let mut index = 0;
        // rposition is the only one here that cannot answer early. std walks it
        // from the back, this walks to the end and keeps the last match, which
        // gives the same index and needs no reversible iterator.
        let mut last_match = None;
        while let Some(value) = self.iterator_next(iterator)? {
            let matches = self
                .call_closure_data(closure, from_ref(&value))?
                .is_truthy();
            match name {
                "find" if matches => return Ok(Value::some(value)),
                "position" if matches => return Ok(Value::some(Value::Int(index))),
                "rposition" if matches => last_match = Some(index),
                "any" if matches => return Ok(Value::Bool(true)),
                "all" if !matches => return Ok(Value::Bool(false)),
                _ => {}
            }
            index += 1;
        }
        Ok(match name {
            "find" | "position" => Value::none(),
            "rposition" => last_match.map_or_else(Value::none, |i| Value::some(Value::Int(i))),
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

/// `sum` over already-drained elements, shared by the lazy iterator path and
/// the eager vec path so both agree on every width.
pub(super) fn sum_values(items: Vec<Value>, target: Option<&ScalarTy>) -> Result<Value> {
    // The accumulator is an i128 so a `u64` element past `i64::MAX` keeps
    // its value. Reading elements through `bridge_image` clamped them to
    // `i64::MAX` before they were even added.
    let mut integers = 0i128;
    // `Sum` for floats starts at -0.0, not 0.0, so that summing negative
    // zeros keeps the sign.
    let mut floats = -0.0f64;
    let mut has_float = false;
    // Without a target the sum is a plain i64 and overflows at its bounds,
    // which is what an untyped `sum()` has always done.
    let (low, high) = match target {
        Some(ScalarTy::Int(width)) => (width.min(), width.max()),
        _ => (i128::from(i64::MIN), i128::from(i64::MAX)),
    };
    for value in items {
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
    let float_target = matches!(target, Some(ScalarTy::F32 | ScalarTy::F64));
    Ok(if has_float || (float_target && integers == 0) {
        // The integer side only joins when it carries a value, so it
        // cannot cancel the -0.0 identity with a +0.0.
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
