//! `flat_map` and `flatten`, lazy like std. The outer item is pulled, and for a `flat_map`
//! its closure called, only when the inner iterator before it is used up.

use std::sync::Arc;

use anyhow::{Result, bail};
use parking_lot::Mutex;

use super::extra::fork_handle;
use super::lending::handed_on;
use super::{Handle, IteratorState, owns_items, take_remaining_of};
use crate::interpreter::enum_def::EnumKind;
use crate::interpreter::native::Native;
use crate::interpreter::value::{ClosureData, Value};
use crate::interpreter::vm::Vm;

/// What the iterator still holds, in the order std drops it: the source, then the inner
/// iterator at the front, then the one at the back.
pub(super) fn remaining(
    source: &Handle,
    front: Option<&Handle>,
    back: Option<&Handle>,
) -> Vec<Value> {
    let mut items = take_remaining_of(source);
    for inner in front.into_iter().chain(back) {
        if owns_items(inner) {
            items.extend(take_remaining_of(inner));
        }
    }
    items
}

impl IteratorState {
    /// The `Clone` of a `flat_map` or a `flatten`, for `cycle`.
    pub(super) fn fork_flatten(&self) -> Result<IteratorState> {
        let IteratorState::Flatten {
            source,
            closure,
            front,
            back,
            owned,
        } = self
        else {
            bail!("not a flattening iterator");
        };
        Ok(IteratorState::Flatten {
            source: fork_handle(source)?,
            closure: closure.clone(),
            front: front.as_ref().map(fork_handle).transpose()?,
            back: back.as_ref().map(fork_handle).transpose()?,
            owned: *owned,
        })
    }
}

/// Which end of the flattened items a pull takes from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum End {
    Front,
    Back,
}

struct Parts {
    source: Handle,
    closure: Option<Arc<ClosureData>>,
    near: Option<Handle>,
    far: Option<Handle>,
}

fn parts(iterator: &Handle, end: End) -> Option<Parts> {
    match &*iterator.lock() {
        Native::Iterator(IteratorState::Flatten {
            source,
            closure,
            front,
            back,
            ..
        }) => {
            let (near, far) = match end {
                End::Front => (front, back),
                End::Back => (back, front),
            };
            Some(Parts {
                source: source.clone(),
                closure: closure.clone(),
                near: near.clone(),
                far: far.clone(),
            })
        }
        _ => None,
    }
}

/// Puts an inner iterator in place at one end, or takes the used up one away.
fn set_inner(iterator: &Handle, end: End, inner: Option<Handle>) {
    if let Native::Iterator(IteratorState::Flatten {
        front, back, owned, ..
    }) = &mut *iterator.lock()
    {
        if let Some(inner) = &inner {
            *owned = owns_items(inner);
        }
        match end {
            End::Front => *front = inner,
            End::Back => *back = inner,
        }
    }
}

impl Vm {
    pub(super) fn flatten_next(self: &Arc<Self>, iterator: &Handle) -> Result<Option<Value>> {
        self.flatten_pull(iterator, End::Front)
    }

    pub(super) fn flatten_next_back(self: &Arc<Self>, iterator: &Handle) -> Result<Option<Value>> {
        self.flatten_pull(iterator, End::Back)
    }

    fn pull_end(self: &Arc<Self>, inner: &Handle, end: End) -> Result<Option<Value>> {
        match end {
            End::Front => self.iterator_next(inner),
            End::Back => self.iterator_next_back(inner),
        }
    }

    /// The inner iterator at this end first, then the next outer item, and last the inner
    /// iterator the other end left unfinished.
    fn flatten_pull(self: &Arc<Self>, iterator: &Handle, end: End) -> Result<Option<Value>> {
        loop {
            let Some(parts) = parts(iterator, end) else {
                return Ok(None);
            };
            if let Some(inner) = &parts.near {
                if let Some(item) = self.pull_end(inner, end)? {
                    return Ok(Some(item));
                }
                set_inner(iterator, end, None);
            }
            let Some(outer) = self.pull_end(&parts.source, end)? else {
                return match &parts.far {
                    Some(inner) => self.pull_end(inner, end),
                    None => Ok(None),
                };
            };
            let inner = self.open_inner(&parts.source, parts.closure.as_ref(), outer)?;
            set_inner(iterator, end, Some(inner));
        }
    }

    /// The iterator over one outer item, or over what the `flat_map` closure makes of it.
    fn open_inner(
        self: &Arc<Self>,
        source: &Handle,
        closure: Option<&Arc<ClosureData>>,
        outer: Value,
    ) -> Result<Handle> {
        let source_owned = owns_items(source);
        let Some(closure) = closure else {
            return self.inner_iterator(outer, source_owned);
        };
        let lent = self.lent_item(&outer, source_owned);
        let made = self.call_closure_with(closure, &[outer], source_owned)?;
        let made = handed_on(made, lent.as_ref());
        // a closure that hands the item on lends it, anything else it made is its own
        let owned = !matches!(made, Value::Ref(_));
        self.inner_iterator(made, owned)
    }

    /// An owned vec or `Option` hands its items over, so one that is thrown away drops. The
    /// items are copied out as handles, the vec itself may still be someone's storage.
    fn inner_iterator(self: &Arc<Self>, value: Value, owned: bool) -> Result<Handle> {
        let values = match &value {
            Value::Vec(items) if owned && !self.has_user_next(&value) => items.lock().clone(),
            Value::Enum { def, .. }
                if owned && matches!(def.kind, EnumKind::Option | EnumKind::Result) =>
            {
                if let Some(payload) = value.success_payload() {
                    vec![payload]
                } else {
                    // an `Err` is consumed with its payload
                    self.run_user_drop(value.clone())?;
                    Vec::new()
                }
            }
            _ => {
                return match self.iterator_value(value)? {
                    Value::Native(inner) => Ok(inner),
                    other => bail!("{} is not iterable", other.type_name()),
                };
            }
        };
        Ok(Arc::new(Mutex::new(Native::Iterator(
            IteratorState::Owned {
                values,
                index: 0,
                vec: true,
            },
        ))))
    }
}
