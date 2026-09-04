//! Values a native method throws away, `unwrap_or` on a `Some` drops its fallback. Real Rust
//! drops them inside the call. A native has no `Vm`, so it parks them here and the method op
//! drops them right after the call, on the same thread.

use std::cell::RefCell;

use super::value::Value;

thread_local! {
    static DISCARDED: RefCell<Vec<Discarded>> = const { RefCell::new(Vec::new()) };
}

pub(super) struct Discarded {
    pub value: Value,
    /// came out of the receiver, not out of an argument
    pub payload: bool,
}

/// A by value argument the native did not hand on. An argument is the caller's to give, so it
/// always drops.
pub(super) fn discard(value: Value) {
    park(value, false);
}

/// A part of the receiver the native threw away, the `Some` payload `filter` rejects. It drops
/// only when the receiver was the caller's own, a handle lent by `v.last()` stays with its
/// owner. Park it after the last closure call of the native, a method op inside the closure
/// drains with its own receiver flag.
pub(super) fn discard_payload(value: Value) {
    park(value, true);
}

fn park(value: Value, payload: bool) {
    DISCARDED.with(|parked| parked.borrow_mut().push(Discarded { value, payload }));
}

/// Everything parked since the last call, in the order it was parked.
pub(super) fn take_discarded() -> Vec<Discarded> {
    DISCARDED.with(|parked| std::mem::take(&mut *parked.borrow_mut()))
}
