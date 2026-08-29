//! Lazy, stateful iterators. An iterator is a shared native handle, so `by_ref`, `peekable` and
//! open ended ranges keep their real semantics.

use std::sync::Arc;

use anyhow::{Result, bail};
use parking_lot::Mutex;

use super::native::Native;
use super::regex_bridge::{CapturesValue, MatchValue, RegexValue};
use super::value::{ClosureData, List, RsStr, Value, ValueRef};

type Handle = Arc<Mutex<Native>>;

pub enum IteratorState {
    Values {
        values: List,
        index: usize,
        /// `into_iter()` on a vec, the source a `collect` can rebuild in place
        owned: bool,
        /// items already handed out by `next_back`
        back: usize,
    },
    MutableValues {
        values: List,
        index: usize,
    },
    /// a user type with its own `Iterator` impl, each pull calls its `next`
    UserNext {
        value: Value,
    },
    /// A `by_ref` borrow of an eager vector. It takes from the front of the shared vector, so
    /// whatever it hands out is gone from the borrowed one too.
    DrainingValues {
        values: List,
    },
    Owned {
        values: Vec<Value>,
        index: usize,
        /// taken from a vec, so a `collect` can rebuild it in place, unlike the items of a map
        vec: bool,
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
    /// `a.zip(b)`
    Zip {
        left: Handle,
        right: Handle,
    },
    /// `a.chain(b)`
    Chain {
        left: Handle,
        right: Handle,
        left_done: bool,
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
    /// `cloned` and `copied`, each item is a deep copy
    Cloned {
        source: Handle,
    },
    Skip {
        source: Handle,
        remaining: usize,
    },
    /// `rev`, each pull is a `next_back` of the source
    Rev {
        source: Handle,
    },
    /// `step_by`
    StepBy {
        source: Handle,
        step: usize,
        first: bool,
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
    /// `peekable`, holds at most 1 item pulled early by `peek`
    Peekable {
        source: Handle,
        buffered: Option<Value>,
    },
}

enum Step {
    Ready(Option<Value>),
    User(Value),
    Map(Handle, Arc<ClosureData>),
    Filter(Handle, Arc<ClosureData>),
    FilterMap(Handle, Arc<ClosureData>),
    Enumerate(Handle, usize),
    Zip(Handle, Handle),
    /// the bool remembers that the left side returned `None`, so it is never asked again
    Chain(Handle, Handle, bool),
    Take(Handle),
    Cloned(Handle),
    Skip(Handle, usize),
    Rev(Handle),
    Stride(Handle, usize),
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
        owned: false,
        back: 0,
    })
}

pub(super) fn owned_iter(items: List) -> Value {
    wrap(IteratorState::Values {
        values: items,
        index: 0,
        owned: true,
        back: 0,
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

/// `peekable` on an eager vector. The buffer sits over a draining view, so `peek` agrees with
/// what `by_ref` on the same vector would hand out.
pub(super) fn peekable_draining(items: List) -> Value {
    let source = Arc::new(Mutex::new(Native::Iterator(
        IteratorState::DrainingValues { values: items },
    )));
    wrap(IteratorState::Peekable {
        source,
        buffered: None,
    })
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
    v.some_payload()
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

/// Shared by the generic step and the scalar for plan, so both walk the source the same way.
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

pub(super) enum FastNext {
    Ready(Option<Value>),
    NotSimple,
}

impl IteratorState {
    /// Produced in place for the simple sources, so a tight loop skips the full `iterator_next`
    /// machinery.
    /// The items an owning iterator still holds, for its drop. A borrowing iterator holds nothing.
    pub(super) fn take_remaining(&mut self) -> Vec<Value> {
        match self {
            IteratorState::Owned { values, index, .. } => values.drain(*index..).collect(),
            _ => Vec::new(),
        }
    }

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

    /// the adaptors with their own state
    fn step_adaptor(&mut self) -> Step {
        match self {
            IteratorState::StepBy {
                source,
                step,
                first,
            } => {
                let skip = if *first { 0 } else { *step - 1 };
                *first = false;
                Step::Stride(source.clone(), skip)
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
            _ => unreachable!("step_adaptor handles the stateful adaptors only"),
        }
    }

    fn step(&mut self) -> Step {
        match self {
            IteratorState::UserNext { value } => Step::User(value.clone()),
            IteratorState::Values {
                values,
                index,
                back,
                ..
            } => {
                let items = values.lock();
                let value = (*index + *back < items.len()).then(|| items[*index].clone());
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
            IteratorState::Owned { values, index, .. } => {
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
            IteratorState::Zip { left, right } => Step::Zip(left.clone(), right.clone()),
            IteratorState::Chain {
                left,
                right,
                left_done,
            } => Step::Chain(left.clone(), right.clone(), *left_done),
            IteratorState::Cloned { source } => Step::Cloned(source.clone()),
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
            IteratorState::Rev { source } => Step::Rev(source.clone()),
            other => other.step_adaptor(),
        }
    }
}

fn bytes_step(source: &str, index: &mut usize) -> Option<Value> {
    let value = source.as_bytes().get(*index).copied();
    *index += usize::from(value.is_some());
    value.map(|byte| Value::Int(i64::from(byte)))
}

fn chars_step(source: &str, offset: &mut usize) -> Option<Value> {
    let value = source[*offset..].chars().next();
    if let Some(ch) = value {
        *offset += ch.len_utf8();
    }
    value.map(Value::Char)
}

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

/// Shared by the generic step and the scalar for plan, so both walk the source the same way.
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

/// Errors wrapped exactly as the `ForNext` op wraps them.
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

fn int_arg(args: &[Value]) -> Result<i64> {
    match args.first() {
        Some(Value::Int(value)) if *value >= 0 => Ok(*value),
        _ => bail!("iterator count needs a non-negative integer"),
    }
}

mod arith;
mod back;
mod drive;
mod in_place;
mod reduce;

pub(super) use arith::{product_values, sum_values};
