//! The storage behind every script map and set. A `BTreeMap` or `BTreeSet` keeps its entries in
//! key order, every other map keeps insertion order.
//!
//! The store hands out only a shared view of the inner `IndexMap`. Every write goes through a
//! method here, so no write can put a key out of order in a sorted store.

use std::cmp::Ordering;
use std::ops::Deref;

use indexmap::IndexMap;
use rustc_hash::FxBuildHasher;

use super::{MapKey, Value};

pub type Entries = IndexMap<MapKey, Value, FxBuildHasher>;

#[derive(Clone, Default)]
pub struct MapStore {
    entries: Entries,
    sorted: bool,
}

impl Deref for MapStore {
    type Target = Entries;

    fn deref(&self) -> &Entries {
        &self.entries
    }
}

impl MapStore {
    /// An empty store in key order, the storage of a `BTreeMap` or `BTreeSet`.
    pub fn sorted() -> Self {
        MapStore {
            entries: Entries::default(),
            sorted: true,
        }
    }

    /// An empty store with the same order rule as `self`.
    pub fn empty_like(&self) -> Self {
        MapStore {
            entries: Entries::default(),
            sorted: self.sorted,
        }
    }

    pub fn is_sorted(&self) -> bool {
        self.sorted
    }

    /// Returns the old value of the key. Like std, a key already present keeps its place and
    /// its original key value.
    pub fn insert(&mut self, key: MapKey, value: Value) -> Option<Value> {
        if !self.sorted {
            return self.entries.insert(key, value);
        }
        match self.entries.binary_search_by(|k, _| k.cmp(&key)) {
            Ok(i) => Some(std::mem::replace(&mut self.entries[i], value)),
            Err(i) => {
                self.entries.shift_insert(i, key, value);
                None
            }
        }
    }

    /// The value of `key`, inserted from `make` first when the key is missing.
    pub fn get_or_insert_with(&mut self, key: MapKey, make: impl FnOnce() -> Value) -> &mut Value {
        if let Some(i) = self.entries.get_index_of(&key) {
            return &mut self.entries[i];
        }
        self.insert(key.clone(), make());
        let index = self
            .entries
            .get_index_of(&key)
            .unwrap_or(self.entries.len() - 1);
        &mut self.entries[index]
    }

    pub fn get_mut(&mut self, key: &MapKey) -> Option<&mut Value> {
        self.entries.get_mut(key)
    }

    pub fn values_mut(&mut self) -> indexmap::map::ValuesMut<'_, MapKey, Value> {
        self.entries.values_mut()
    }

    /// Keeps the order of the rest, so a sorted store stays sorted.
    pub fn shift_remove(&mut self, key: &MapKey) -> Option<Value> {
        self.entries.shift_remove(key)
    }

    pub fn shift_remove_entry(&mut self, key: &MapKey) -> Option<(MapKey, Value)> {
        self.entries.shift_remove_entry(key)
    }

    pub fn shift_remove_index(&mut self, index: usize) -> Option<(MapKey, Value)> {
        self.entries.shift_remove_index(index)
    }

    pub fn pop(&mut self) -> Option<(MapKey, Value)> {
        self.entries.pop()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn retain(&mut self, keep: impl FnMut(&MapKey, &mut Value) -> bool) {
        self.entries.retain(keep);
    }

    /// Takes every entry out and leaves an empty store with the same order rule.
    pub fn take_all(&mut self) -> Entries {
        std::mem::take(&mut self.entries)
    }

    pub fn into_entries(self) -> Entries {
        self.entries
    }

    /// The first entry of a key range in the store's order, for `range` on a sorted store.
    pub fn lower_bound(&self, key: &MapKey, inclusive: bool) -> usize {
        self.entries.partition_point(|k, _| match k.cmp(key) {
            Ordering::Less => true,
            Ordering::Equal => !inclusive,
            Ordering::Greater => false,
        })
    }
}

impl Extend<(MapKey, Value)> for MapStore {
    fn extend<I: IntoIterator<Item = (MapKey, Value)>>(&mut self, items: I) {
        for (key, value) in items {
            self.insert(key, value);
        }
    }
}

impl FromIterator<(MapKey, Value)> for MapStore {
    fn from_iter<I: IntoIterator<Item = (MapKey, Value)>>(items: I) -> Self {
        MapStore {
            entries: items.into_iter().collect(),
            sorted: false,
        }
    }
}

impl IntoIterator for MapStore {
    type Item = (MapKey, Value);
    type IntoIter = indexmap::map::IntoIter<MapKey, Value>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl<'a> IntoIterator for &'a MapStore {
    type Item = (&'a MapKey, &'a Value);
    type IntoIter = indexmap::map::Iter<'a, MapKey, Value>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}
