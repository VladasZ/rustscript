//! `ValueRef`, the place a `&mut` borrow points at.

use std::sync::Arc;

use parking_lot::Mutex;

use super::{List, Map, MapKey, StructData, Value};
use crate::interpreter::borrow::BorrowGuard;

pub enum ValueRef {
    VecElement {
        values: List,
        index: usize,
    },
    MapEntry {
        map: Map,
        key: MapKey,
    },
    StructField {
        data: Arc<StructData>,
        slot: usize,
    },
    /// from `borrow`, `borrow_mut` and `lock`
    CellSlot {
        slot: Arc<Mutex<Value>>,
        /// the live `RefCell` borrow, `None` for a `lock`
        guard: Option<Arc<BorrowGuard>>,
    },
    /// From accessors like `as_object_mut`. No mutation split here, only in place mutation goes through.
    Borrowed {
        value: Value,
    },
}

impl ValueRef {
    pub fn vec_element(values: List, index: usize) -> Self {
        Self::VecElement { values, index }
    }

    pub fn map_entry(map: Map, key: MapKey) -> Self {
        Self::MapEntry { map, key }
    }

    pub fn struct_field(data: Arc<StructData>, slot: usize) -> Self {
        Self::StructField { data, slot }
    }

    pub fn cell_slot(slot: Arc<Mutex<Value>>) -> Self {
        Self::CellSlot { slot, guard: None }
    }

    pub fn borrowed_cell_slot(slot: Arc<Mutex<Value>>, guard: Arc<BorrowGuard>) -> Self {
        Self::CellSlot {
            slot,
            guard: Some(guard),
        }
    }

    pub fn borrowed(value: Value) -> Self {
        Self::Borrowed { value }
    }

    pub fn get(&self) -> Option<Value> {
        match self {
            Self::VecElement { values, index } => values.lock().get(*index).cloned(),
            Self::MapEntry { map, key } => map.lock().get(key).cloned(),
            Self::StructField { data, slot } => data.values.lock().get(*slot).cloned(),
            Self::CellSlot { slot, .. } => Some(slot.lock().clone()),
            Self::Borrowed { value } => Some(value.clone()),
        }
    }

    /// One atomic read-modify-write under the slot lock. `None` for a dangling reference or a
    /// `Borrowed` view, then the caller runs the unfused sequence. `f` must not run user code or
    /// lock other values.
    pub fn update<T>(&self, f: impl FnOnce(&mut Value) -> T) -> Option<T> {
        match self {
            Self::VecElement { values, index } => values.lock().get_mut(*index).map(f),
            Self::MapEntry { map, key } => map.lock().get_mut(key).map(f),
            Self::StructField { data, slot } => data.values.lock().get_mut(*slot).map(f),
            Self::CellSlot { slot, .. } => Some(f(&mut slot.lock())),
            Self::Borrowed { .. } => None,
        }
    }

    /// A shared `borrow()` cannot be written through. Valid Rust never tries, so this only
    /// shows for a script that skipped `rust check`.
    pub fn writable(&self) -> bool {
        match self {
            Self::CellSlot {
                guard: Some(guard), ..
            } => guard.is_exclusive(),
            _ => true,
        }
    }

    pub fn set(&self, value: Value) -> bool {
        match self {
            Self::VecElement { values, index } => {
                let mut values = values.lock();
                let Some(slot) = values.get_mut(*index) else {
                    return false;
                };
                *slot = value;
                true
            }
            Self::MapEntry { map, key } => {
                map.lock().insert(key.clone(), value);
                true
            }
            Self::StructField { data, slot } => {
                let mut values = data.values.lock();
                let Some(target) = values.get_mut(*slot) else {
                    return false;
                };
                *target = value;
                true
            }
            Self::CellSlot { slot, .. } => {
                *slot.lock() = value;
                true
            }
            Self::Borrowed { .. } => false,
        }
    }
}
