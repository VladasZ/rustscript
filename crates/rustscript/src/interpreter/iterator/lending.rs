//! References an iterator hands on. A chain passes a `&T` item on as the plain handle of the
//! item. A collection that keeps such an item holds a reference, so the item goes in behind a
//! borrow that drops nothing, and the owner keeps it.

use std::sync::Arc;

use anyhow::Result;

use super::drive::lend;
use super::{Handle, IteratorState, lends_vec_items};
use crate::interpreter::native::Native;
use crate::interpreter::value::Value;
use crate::interpreter::vm::Vm;

/// Which side of the pairs is a reference into a vec, the left and the right one of a `zip`
/// and the right one of an `enumerate`.
fn lent_sides(handle: &Handle) -> (bool, bool) {
    match &*handle.lock() {
        Native::Iterator(IteratorState::Zip { left, right }) => {
            (lends_vec_items(left), lends_vec_items(right))
        }
        Native::Iterator(IteratorState::Enumerate { source, .. }) => {
            (false, lends_vec_items(source))
        }
        Native::Iterator(
            IteratorState::Filter { source, .. }
            | IteratorState::Inspect { source, .. }
            | IteratorState::Take { source, .. }
            | IteratorState::Skip { source, .. }
            | IteratorState::Rev { source }
            | IteratorState::StepBy { source, .. }
            | IteratorState::TakeWhile { source, .. }
            | IteratorState::SkipWhile { source, .. }
            | IteratorState::ByRef { source }
            | IteratorState::Peekable { source, .. },
        ) => lent_sides(source),
        _ => (false, false),
    }
}

/// What a `map` or `filter_map` closure made of the item a lending source handed it, `None`
/// for an owning source. A closure that hands the item on, `|t| t` or `|(_, t)| t`, made
/// a reference to it. That goes on behind a borrow, so nothing down the chain drops it.
pub(super) fn handed_on(out: Value, item: Option<&Value>) -> Value {
    let Some(item) = item else {
        return out;
    };
    let same = |part: &Value| out.shares_storage(part);
    let lent = same(item) || matches!(item, Value::Tuple(parts) if parts.lock().iter().any(same));
    if lent { lend(&out) } else { out }
}

/// The item `cloned` makes. An owning source hands the reference itself on, the `&T` of
/// `held.into_iter()` over a `Vec<&T>`, so the clone is of the value behind it. A lending
/// source hands on a reference to that reference, and its clone is the reference.
pub(super) fn cloned_item(item: &Value, owned: bool) -> Value {
    match item {
        Value::Ref(reference) if owned => reference
            .lent()
            .map_or_else(|| item.deep_clone(), |inner| inner.deep_clone()),
        _ => item.deep_clone(),
    }
}

impl Vm {
    /// The item a lending source hands to a closure, kept for `handed_on`. Only a program
    /// with a `Drop` impl can tell who owns it.
    pub(super) fn lent_item(&self, item: &Value, owned: bool) -> Option<Value> {
        (self.has_drop && !owned).then(|| item.clone())
    }

    /// The items for a collection. One that is a reference into a vec goes in behind a borrow,
    /// also as a side of a pair, so the collection drops nothing of the vec. `whole` is off
    /// for a `Vec`, whose binding the compiler already keeps from dropping its items.
    pub(super) fn drain_lending(
        self: &Arc<Self>,
        iterator: &Handle,
        whole: bool,
    ) -> Result<Vec<Value>> {
        if !self.has_drop {
            return self.drain_iterator(iterator);
        }
        let whole = whole && lends_vec_items(iterator);
        let (left, right) = lent_sides(iterator);
        let mut items = self.drain_iterator(iterator)?;
        for item in &mut items {
            if whole {
                *item = lend(item);
            } else if let Value::Tuple(pair) = item
                && let [first, second] = pair.lock().as_mut_slice()
            {
                if left {
                    *first = lend(first);
                }
                if right {
                    *second = lend(second);
                }
            }
        }
        Ok(items)
    }

    /// The items of an `extend` argument, see `drain_lending`.
    pub(in crate::interpreter) fn drain_extend_items(
        self: &Arc<Self>,
        value: Value,
    ) -> Result<Vec<Value>> {
        let Value::Native(iterator) = self.iterator_value(value)? else {
            unreachable!();
        };
        self.drain_lending(&iterator, true)
    }
}
