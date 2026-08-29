//! The double ended side. `rev` is lazy like std, so a `map` closure runs from the back and a
//! `skip` after a `rev` never touches what it skips. A source without a back step, a `filter_map`
//! over a regex say, still drains eagerly.

use std::slice::from_ref;
use std::sync::Arc;

use anyhow::{Result, bail};

use super::{Handle, IteratorState, option_inner};
use crate::interpreter::native::Native;
use crate::interpreter::shared::usize_i64;
use crate::interpreter::value::{ClosureData, Value};
use crate::interpreter::vm::Vm;

enum Back {
    Ready(Option<Value>),
    Map(Handle, Arc<ClosureData>),
    Filter(Handle, Arc<ClosureData>),
    FilterMap(Handle, Arc<ClosureData>),
    Cloned(Handle),
    Enumerate(Handle, usize),
    Skip(Handle, usize),
    Take(Handle, usize),
    Chain(Handle, Handle),
    Zip(Handle, Handle),
    Forward(Handle),
}

/// Whether every layer down to the source has a back step.
pub(super) fn supports_back(iterator: &Handle) -> bool {
    let guard = iterator.lock();
    let Native::Iterator(state) = &*guard else {
        return false;
    };
    match state {
        IteratorState::Values { .. }
        | IteratorState::Owned { .. }
        | IteratorState::Range { .. } => true,
        IteratorState::Map { source, .. }
        | IteratorState::Filter { source, .. }
        | IteratorState::FilterMap { source, .. }
        | IteratorState::Cloned { source }
        | IteratorState::Rev { source } => supports_back(source),
        // these need the exact length of what is left
        IteratorState::Enumerate { source, .. }
        | IteratorState::Skip { source, .. }
        | IteratorState::Take { source, .. } => {
            supports_back(source) && iterator_len(source).is_some()
        }
        IteratorState::Chain { left, right, .. } => supports_back(left) && supports_back(right),
        IteratorState::Zip { left, right } => {
            supports_back(left)
                && supports_back(right)
                && iterator_len(left).is_some()
                && iterator_len(right).is_some()
        }
        _ => false,
    }
}

/// The exact number of items left, `None` when the shape cannot know.
pub(super) fn iterator_len(iterator: &Handle) -> Option<usize> {
    let guard = iterator.lock();
    let Native::Iterator(state) = &*guard else {
        return None;
    };
    match state {
        IteratorState::Values {
            values,
            index,
            back,
            ..
        } => Some(values.lock().len().saturating_sub(*index + *back)),
        IteratorState::Owned { values, index, .. } => Some(values.len() - *index),
        IteratorState::Range {
            next,
            end,
            inclusive,
        } => {
            let span = i128::from(*end) - i128::from(*next) + i128::from(*inclusive);
            usize::try_from(span.max(0)).ok()
        }
        IteratorState::Map { source, .. }
        | IteratorState::Cloned { source }
        | IteratorState::Enumerate { source, .. }
        | IteratorState::Rev { source } => iterator_len(source),
        IteratorState::Skip { source, remaining } => {
            Some(iterator_len(source)?.saturating_sub(*remaining))
        }
        IteratorState::Take { source, remaining } => Some(iterator_len(source)?.min(*remaining)),
        IteratorState::Zip { left, right } => Some(iterator_len(left)?.min(iterator_len(right)?)),
        IteratorState::Chain { left, right, .. } => {
            Some(iterator_len(left)? + iterator_len(right)?)
        }
        _ => None,
    }
}

/// The sources answer in place, the adaptors hand back what to pull from.
fn back_step(state: &mut IteratorState) -> Result<Back> {
    Ok(match state {
        IteratorState::Values {
            values,
            index,
            back,
            ..
        } => {
            let items = values.lock();
            let end = items.len().saturating_sub(*back);
            let value = (end > *index).then(|| items[end - 1].clone());
            *back += usize::from(value.is_some());
            Back::Ready(value)
        }
        IteratorState::Owned { values, index, .. } => {
            Back::Ready((values.len() > *index).then(|| values.pop()).flatten())
        }
        IteratorState::Range {
            next,
            end,
            inclusive,
        } => Back::Ready(range_back(next, end, *inclusive)),
        IteratorState::Map { source, closure } => Back::Map(source.clone(), closure.clone()),
        IteratorState::Filter { source, closure } => Back::Filter(source.clone(), closure.clone()),
        IteratorState::FilterMap { source, closure } => {
            Back::FilterMap(source.clone(), closure.clone())
        }
        IteratorState::Cloned { source } => Back::Cloned(source.clone()),
        IteratorState::Enumerate { source, index } => Back::Enumerate(source.clone(), *index),
        IteratorState::Skip { source, remaining } => Back::Skip(source.clone(), *remaining),
        IteratorState::Take { source, remaining } => {
            if *remaining == 0 {
                Back::Ready(None)
            } else {
                let count = *remaining;
                *remaining -= 1;
                Back::Take(source.clone(), count)
            }
        }
        IteratorState::Chain { left, right, .. } => Back::Chain(left.clone(), right.clone()),
        IteratorState::Zip { left, right } => Back::Zip(left.clone(), right.clone()),
        IteratorState::Rev { source } => Back::Forward(source.clone()),
        _ => bail!("this iterator cannot be reversed"),
    })
}

fn sized(source: &Handle, what: &str) -> Result<usize> {
    match iterator_len(source) {
        Some(len) => Ok(len),
        None => bail!("{what} cannot be reversed here"),
    }
}

impl Vm {
    pub(super) fn iterator_next_back(self: &Arc<Self>, iterator: &Handle) -> Result<Option<Value>> {
        let back = {
            let mut guard = iterator.lock();
            let Native::Iterator(state) = &mut *guard else {
                bail!("{} is not an iterator", guard.type_name());
            };
            back_step(state)?
        };
        match back {
            Back::Ready(value) => Ok(value),
            Back::Map(source, closure) => match self.iterator_next_back(&source)? {
                Some(value) => Ok(Some(self.call_closure_data(&closure, &[value])?)),
                None => Ok(None),
            },
            Back::Filter(source, closure) => loop {
                let Some(value) = self.iterator_next_back(&source)? else {
                    return Ok(None);
                };
                if self
                    .call_closure_data(&closure, from_ref(&value))?
                    .is_truthy()
                {
                    return Ok(Some(value));
                }
            },
            Back::FilterMap(source, closure) => loop {
                let Some(value) = self.iterator_next_back(&source)? else {
                    return Ok(None);
                };
                if let Some(inner) = option_inner(&self.call_closure_data(&closure, &[value])?) {
                    return Ok(Some(inner));
                }
            },
            Back::Cloned(source) => Ok(self.iterator_next_back(&source)?.map(|v| v.deep_clone())),
            Back::Forward(source) => self.iterator_next(&source),
            Back::Chain(left, right) => match self.iterator_next_back(&right)? {
                Some(value) => Ok(Some(value)),
                None => self.iterator_next_back(&left),
            },
            Back::Enumerate(source, index) => self.enumerate_back(&source, index),
            Back::Skip(source, remaining) => self.skip_back(&source, remaining),
            Back::Take(source, count) => self.take_back(&source, count),
            Back::Zip(left, right) => self.zip_back(&left, &right),
        }
    }

    /// The index of the last item is the count already handed out plus what is left.
    fn enumerate_back(self: &Arc<Self>, source: &Handle, index: usize) -> Result<Option<Value>> {
        let len = sized(source, "enumerate")?;
        Ok(self
            .iterator_next_back(source)?
            .map(|value| Value::tuple(vec![Value::Int(usize_i64(index + len - 1)), value])))
    }

    /// `Skip::next_back` only answers while items past the skipped prefix remain.
    fn skip_back(self: &Arc<Self>, source: &Handle, remaining: usize) -> Result<Option<Value>> {
        if sized(source, "skip")? > remaining {
            self.iterator_next_back(source)
        } else {
            Ok(None)
        }
    }

    /// `Take::next_back` drops the tail past its window first, like `nth_back`.
    fn take_back(self: &Arc<Self>, source: &Handle, count: usize) -> Result<Option<Value>> {
        let len = sized(source, "take")?;
        for _ in 0..len.saturating_sub(count) {
            if self.iterator_next_back(source)?.is_none() {
                return Ok(None);
            }
        }
        self.iterator_next_back(source)
    }

    /// `Zip::next_back` trims the longer side to the shorter before pairing from the back.
    fn zip_back(self: &Arc<Self>, left: &Handle, right: &Handle) -> Result<Option<Value>> {
        let (a, b) = (sized(left, "zip")?, sized(right, "zip")?);
        for _ in b..a {
            self.iterator_next_back(left)?;
        }
        for _ in a..b {
            self.iterator_next_back(right)?;
        }
        match (
            self.iterator_next_back(left)?,
            self.iterator_next_back(right)?,
        ) {
            (Some(a), Some(b)) => Ok(Some(Value::tuple(vec![a, b]))),
            _ => Ok(None),
        }
    }
}

fn range_back(next: &mut i64, end: &mut i64, inclusive: bool) -> Option<Value> {
    if inclusive {
        if *next > *end {
            return None;
        }
        let value = *end;
        // an inclusive range at `i64::MIN` cannot step below it, so it empties from the front
        if value == i64::MIN {
            *next = i64::MAX;
            *end = i64::MIN;
        } else {
            *end -= 1;
        }
        Some(Value::Int(value))
    } else {
        if *next >= *end {
            return None;
        }
        *end -= 1;
        Some(Value::Int(*end))
    }
}
