//! `RefCell` borrow tracking. Real Rust refuses a second live `borrow_mut`, so a script that
//! holds two guards must panic here too, with the std messages. The live borrows of every cell
//! sit in one table keyed by the slot address, and a guard removes itself when its last holder
//! drops. A guard only exists inside a `ValueRef::CellSlot` that also owns the slot, so the
//! address cannot be reused while the entry is live.

use std::sync::{Arc, LazyLock};

use parking_lot::Mutex;
use rustc_hash::FxHashMap;

use super::native::Native;
use super::value::Value;

#[derive(Default)]
struct State {
    shared: usize,
    exclusive: bool,
}

static BORROWS: LazyLock<Mutex<FxHashMap<usize, State>>> = LazyLock::new(Mutex::default);

fn key(slot: &Arc<Mutex<Value>>) -> usize {
    Arc::as_ptr(slot) as usize
}

/// One live borrow of a cell. Dropping it ends the borrow.
pub struct BorrowGuard {
    key: usize,
    exclusive: bool,
}

impl BorrowGuard {
    pub fn is_exclusive(&self) -> bool {
        self.exclusive
    }
}

impl Drop for BorrowGuard {
    fn drop(&mut self) {
        let mut table = BORROWS.lock();
        let Some(state) = table.get_mut(&self.key) else {
            return;
        };
        if self.exclusive {
            state.exclusive = false;
        } else {
            state.shared = state.shared.saturating_sub(1);
        }
        if !state.exclusive && state.shared == 0 {
            table.remove(&self.key);
        }
    }
}

/// What `borrow` or `borrow_mut` hit, the std `BorrowError` or `BorrowMutError`.
pub struct BorrowFailure {
    exclusive: bool,
}

impl BorrowFailure {
    /// The std panic text, which is the `Display` text since rustc 1.96.
    pub fn message(&self) -> String {
        self.display().to_string()
    }

    fn display(&self) -> &'static str {
        if self.exclusive {
            "RefCell already borrowed"
        } else {
            "RefCell already mutably borrowed"
        }
    }

    fn debug(&self) -> &'static str {
        if self.exclusive {
            "BorrowMutError"
        } else {
            "BorrowError"
        }
    }

    /// The `Err` payload of `try_borrow` and `try_borrow_mut`.
    pub fn value(&self) -> Value {
        Native::ParseErr {
            display: self.display().to_string(),
            debug: self.debug().to_string(),
        }
        .wrap()
    }
}

/// Registers a borrow of `slot`, exclusive for `borrow_mut`.
pub fn acquire(
    slot: &Arc<Mutex<Value>>,
    exclusive: bool,
) -> Result<Arc<BorrowGuard>, BorrowFailure> {
    let key = key(slot);
    let mut table = BORROWS.lock();
    let state = table.entry(key).or_default();
    let blocked = state.exclusive || (exclusive && state.shared > 0);
    if blocked {
        if !state.exclusive && state.shared == 0 {
            table.remove(&key);
        }
        return Err(BorrowFailure { exclusive });
    }
    if exclusive {
        state.exclusive = true;
    } else {
        state.shared += 1;
    }
    Ok(Arc::new(BorrowGuard { key, exclusive }))
}
