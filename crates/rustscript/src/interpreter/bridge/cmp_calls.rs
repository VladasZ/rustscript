//! `std::cmp::max` and `min` with their `_by` and `_by_key` forms. Each hands one of its 2
//! arguments back as it came, so the value keeps its width tag.

use std::cmp::Ordering;
use std::slice::from_ref;
use std::sync::Arc;

use anyhow::{Result, bail};

use super::arg;
use crate::interpreter::bytecode::{BuiltinId, Chunk, PathId};
use crate::interpreter::iterator::as_closure;
use crate::interpreter::methods::ordering_from_value;
use crate::interpreter::ops::partial_compare;
use crate::interpreter::value::Value;
use crate::interpreter::vm::Vm;

impl Vm {
    /// `None` for any other path. The questions are the ones std asks, in its order, so a
    /// closure that prints shows the same lines and equal values pick the same side, the
    /// second for a `max` and the first for a `min`.
    pub(super) fn cmp_call(self: &Arc<Self>, id: PathId, args: &[Value]) -> Result<Option<Value>> {
        let want_max = match id {
            PathId::CmpMax | PathId::CmpMaxBy | PathId::CmpMaxByKey => true,
            PathId::CmpMin | PathId::CmpMinBy | PathId::CmpMinByKey => false,
            _ => return Ok(None),
        };
        let (first, second) = (arg(args, 0)?, arg(args, 1)?);
        let first_is_greater = match id {
            PathId::CmpMax | PathId::CmpMin => self.is_less(&second, &first)?,
            PathId::CmpMaxBy | PathId::CmpMinBy => {
                let compare = as_closure(args.get(2))?;
                let order = self.call_closure_data(&compare, &[first.clone(), second.clone()])?;
                match ordering_from_value(&order) {
                    Some(order) => order == Ordering::Greater,
                    None => bail!("the closure of a `_by` call must return an Ordering"),
                }
            }
            _ => {
                let key = as_closure(args.get(2))?;
                let second_key = self.call_closure_data(&key, from_ref(&second))?;
                let first_key = self.call_closure_data(&key, from_ref(&first))?;
                let less = self.is_less(&second_key, &first_key)?;
                if self.has_drop {
                    self.run_user_drop(first_key)?;
                    self.run_user_drop(second_key)?;
                }
                less
            }
        };
        let (winner, loser) = if first_is_greater == want_max {
            (first, second)
        } else {
            (second, first)
        };
        if self.has_drop {
            self.run_user_drop(loser)?;
        }
        Ok(Some(winner))
    }

    /// `left.partial_cmp(right)` the way the type says it. A hand written `partial_cmp` of a
    /// script type runs, and it is what the provided `max`, `min` and `clamp` of std call, so
    /// what it prints is output. Any other value orders by its parts.
    fn order_of(self: &Arc<Self>, left: &Value, right: &Value) -> Result<Option<Ordering>> {
        let (left, right) = (referent(left), referent(right));
        let Some(chunk) = self.written_partial_cmp(&left) else {
            return partial_compare(&left, &right);
        };
        let answer = self.run_chunk(&chunk, &[left, right], &[], false)?;
        Ok(answer.some_payload().as_ref().and_then(ordering_from_value))
    }

    fn is_less(self: &Arc<Self>, left: &Value, right: &Value) -> Result<bool> {
        Ok(self.order_of(left, right)? == Some(Ordering::Less))
    }

    fn written_partial_cmp(&self, value: &Value) -> Option<Arc<Chunk>> {
        if self.impls.is_empty() {
            return None;
        }
        self.impls
            .of_value(value)
            .and_then(|methods| methods.builtin(BuiltinId::PartialCmp))
            .cloned()
    }

    /// `max`, `min` and `clamp` as methods on a script type with a hand written `partial_cmp`,
    /// with the questions std asks, in its order. `clamp` also covers a script type and a tuple
    /// that order by their parts. `None` leaves the call to the builtin of the receiver.
    pub(super) fn ord_method(
        self: &Arc<Self>,
        recv: &Value,
        id: BuiltinId,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let by_parts = matches!(
            recv,
            Value::Struct(_) | Value::Enum { .. } | Value::Tuple(_)
        );
        let clamps = id == BuiltinId::Clamp && by_parts && args.len() == 2;
        if !clamps && (args.len() != 1 || self.written_partial_cmp(recv).is_none()) {
            return Ok(None);
        }
        let other = arg(args, 0)?;
        if clamps {
            let high = arg(args, 1)?;
            if !matches!(
                self.order_of(&other, &high)?,
                Some(Ordering::Less | Ordering::Equal)
            ) {
                bail!("assertion failed: min <= max");
            }
            return Ok(Some(if self.is_less(recv, &other)? {
                other
            } else if self.order_of(recv, &high)? == Some(Ordering::Greater) {
                high
            } else {
                recv.clone()
            }));
        }
        let other_is_less = self.is_less(&other, recv)?;
        Ok(Some(if other_is_less == (id == BuiltinId::Max) {
            recv.clone()
        } else {
            other
        }))
    }
}

/// The value behind a borrow, any other value as it is.
fn referent(value: &Value) -> Value {
    match value {
        Value::Ref(reference) => reference.get().unwrap_or_else(|| value.clone()),
        other => other.clone(),
    }
}
