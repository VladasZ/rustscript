//! The map and set methods. A `HashMap` keeps insertion order and a `BTreeMap` key order, see
//! `MapStore`, and a set is a map with unit values.

use std::mem::take;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use parking_lot::Mutex;

use super::bytecode::{BuiltinId, MethodName};
use super::discard::discard;
use super::iterator;
use super::native::Native;
use super::value::{MapKey, MapKind, MapStore, Value};

/// A key the map throws away. It is the map's own, so it drops inside the call like in real
/// Rust. Only a user type can have a `Drop` impl.
fn discard_key(key: &MapKey) {
    if key.may_drop() {
        discard(key.to_value());
    }
}

pub(super) fn map_method(
    m: &Arc<Mutex<MapStore>>,
    kind: MapKind,
    method: &MethodName,
    args: &mut [Value],
) -> Result<Value> {
    let lookup = |i: usize, f: &dyn Fn(Option<&Value>) -> Value| -> Result<Value> {
        let arg = args.get(i).ok_or_else(|| anyhow!("invalid map key"))?;
        let k = arg.as_key().ok_or_else(|| anyhow!("invalid map key"))?;
        Ok(f(m.lock().get(&k)))
    };
    if let Some(out) = set_or_sorted_method(m, kind, method, args)? {
        return Ok(out);
    }
    if let Some(out) = map_write_method(m, kind, method, args)? {
        return Ok(out);
    }
    Ok(match method.id {
        BuiltinId::Len | BuiltinId::Count => super::shared::usize_value(m.lock().len()),
        BuiltinId::IsEmpty => Value::Bool(m.lock().is_empty()),
        BuiltinId::Clone => Value::Map(m.clone(), kind).deep_clone(),
        BuiltinId::Insert => return map_insert(m, kind, args),
        // `get_mut` is `&mut V`, so writes must land in the entry
        BuiltinId::GetMut => {
            let arg = args.first().ok_or_else(|| anyhow!("invalid map key"))?;
            let k = arg.as_key().ok_or_else(|| anyhow!("invalid map key"))?;
            if m.lock().contains_key(&k) {
                Value::some(Value::Ref(Arc::new(super::value::ValueRef::map_entry(
                    m.clone(),
                    k,
                ))))
            } else {
                Value::none()
            }
        }
        BuiltinId::Get => lookup(0, &|v| match v {
            Some(v) => Value::some(v.clone()),
            None => Value::none(),
        })?,
        BuiltinId::ContainsKey => lookup(0, &|v| Value::Bool(v.is_some()))?,
        BuiltinId::Remove => return map_remove(m, kind, args),
        BuiltinId::IntoKeys => map_into_iterator(m, true),
        BuiltinId::IntoValues => map_into_iterator(m, false),
        BuiltinId::Keys => Value::vec(m.lock().keys().map(MapKey::to_value).collect()),
        BuiltinId::Values => Value::vec(m.lock().values().cloned().collect()),
        BuiltinId::Entry => {
            let Some(key) = args.first().and_then(Value::as_key) else {
                bail!("invalid entry key");
            };
            Native::Entry {
                map: m.clone(),
                key,
            }
            .wrap()
        }
        BuiltinId::Clear => {
            let entries = m.lock().take_all();
            for (key, value) in entries {
                discard_key(&key);
                discard(value);
            }
            Value::Unit
        }
        // `drain` hands every entry out and leaves the map empty
        BuiltinId::Drain => {
            let drained = if kind == MapKind::Set {
                set_items(m)
            } else {
                map_pairs(m)
            };
            m.lock().clear();
            drained
        }
        BuiltinId::Iter | BuiltinId::IntoIter if kind == MapKind::Set => set_items(m),
        BuiltinId::Iter | BuiltinId::IntoIter => map_pairs(m),
        // a parsed json object is an Arc shared Map, so the mut accessor hands back the same map
        BuiltinId::AsObject => Value::some(Value::Map(m.clone(), kind)),
        BuiltinId::AsObjectMut => Value::some(Value::Ref(Arc::new(
            super::value::ValueRef::borrowed(Value::Map(m.clone(), kind)),
        ))),
        BuiltinId::AsArray | BuiltinId::AsArrayMut => Value::none(),
        _ => return super::methods::generic_method(&Value::Map(m.clone(), kind), method, args),
    })
}

fn map_insert(m: &Arc<Mutex<MapStore>>, kind: MapKind, args: &mut [Value]) -> Result<Value> {
    let k = take(&mut args[0])
        .into_key()
        .ok_or_else(|| anyhow!("invalid map key"))?;
    // std keeps the key it has and drops the one handed in
    let present = m.lock().contains_key(&k);
    if present {
        discard_key(&k);
    }
    // a set insert returns whether it was new, a map insert the old value
    if kind == MapKind::Set {
        if !present {
            m.lock().insert(k, Value::Unit);
        }
        return Ok(Value::Bool(!present));
    }
    let val = args.get_mut(1).map_or(Value::Unit, take);
    let old = m.lock().insert(k, val);
    Ok(match old {
        Some(v) => Value::some(v),
        None => Value::none(),
    })
}

fn map_remove(m: &Arc<Mutex<MapStore>>, kind: MapKind, args: &[Value]) -> Result<Value> {
    let arg = args.first().ok_or_else(|| anyhow!("invalid map key"))?;
    let k = arg.as_key().ok_or_else(|| anyhow!("invalid map key"))?;
    let removed = m.lock().shift_remove_entry(&k);
    if let Some((key, _)) = &removed {
        discard_key(key);
    }
    if kind == MapKind::Set {
        return Ok(Value::Bool(removed.is_some()));
    }
    Ok(match removed {
        Some((_, v)) => Value::some(v),
        None => Value::none(),
    })
}

/// The combinations iterate this set's elements then the other's. Real Rust doesn't promise any
/// order here.
fn set_relation(m: &Arc<Mutex<MapStore>>, id: BuiltinId, args: &[Value]) -> Result<Value> {
    // a set of references sits behind a borrow, see `keep_lent`
    let other = match args.first() {
        Some(Value::Ref(reference)) => reference.get(),
        other => other.cloned(),
    };
    let Some(Value::Map(other, MapKind::Set)) = other else {
        bail!("set operation needs a set argument");
    };
    // snapshots, a set compared with itself would relock
    let mine: Vec<MapKey> = m.lock().keys().cloned().collect();
    let theirs: MapStore = other.lock().clone();
    let has = |k: &MapKey| theirs.contains_key(k);
    // a `BTreeSet` hands every combination out in ascending order
    let sorted = m.lock().is_sorted();
    let elems = |mut keys: Vec<MapKey>| {
        if sorted {
            keys.sort();
        }
        iterator::value_iter(Arc::new(Mutex::new(
            keys.iter().map(MapKey::to_value).collect(),
        )))
    };
    Ok(match id {
        BuiltinId::IsSubset => Value::Bool(mine.iter().all(has)),
        BuiltinId::IsSuperset => Value::Bool(theirs.keys().all(|k| mine.contains(k))),
        BuiltinId::IsDisjoint => Value::Bool(!mine.iter().any(has)),
        BuiltinId::Union => {
            let mut keys = mine.clone();
            keys.extend(theirs.keys().filter(|k| !mine.contains(k)).cloned());
            elems(keys)
        }
        BuiltinId::Intersection => elems(mine.into_iter().filter(|k| has(k)).collect()),
        BuiltinId::Difference => elems(mine.into_iter().filter(|k| !has(k)).collect()),
        _ => {
            let mut keys: Vec<MapKey> = mine.iter().filter(|k| !has(k)).cloned().collect();
            keys.extend(theirs.keys().filter(|k| !mine.contains(k)).cloned());
            elems(keys)
        }
    })
}

/// The methods only a set or only a `BTreeMap` has, tried before the shared ones.
fn set_or_sorted_method(
    m: &Arc<Mutex<MapStore>>,
    kind: MapKind,
    method: &MethodName,
    args: &[Value],
) -> Result<Option<Value>> {
    Ok(Some(match method.id {
        // a set `get` returns the element, not the Unit that backs it
        BuiltinId::Get if kind == MapKind::Set => {
            let arg = args.first().ok_or_else(|| anyhow!("invalid map key"))?;
            let k = arg.as_key().ok_or_else(|| anyhow!("invalid map key"))?;
            match m.lock().get_key_value(&k) {
                Some((key, _)) => Value::some(key.to_value()),
                None => Value::none(),
            }
        }
        BuiltinId::Contains if kind == MapKind::Set => {
            let arg = args.first().ok_or_else(|| anyhow!("invalid map key"))?;
            let k = arg.as_key().ok_or_else(|| anyhow!("invalid map key"))?;
            Value::Bool(m.lock().contains_key(&k))
        }
        BuiltinId::IsSubset
        | BuiltinId::IsSuperset
        | BuiltinId::IsDisjoint
        | BuiltinId::Union
        | BuiltinId::Intersection
        | BuiltinId::Difference
        | BuiltinId::SymmetricDifference
            if kind == MapKind::Set =>
        {
            set_relation(m, method.id, args)?
        }
        BuiltinId::FirstKeyValue
        | BuiltinId::LastKeyValue
        | BuiltinId::PopFirst
        | BuiltinId::PopLast => sorted_ends(m, kind, method.id),
        BuiltinId::First | BuiltinId::Last if kind == MapKind::Set => {
            sorted_ends(m, kind, method.id)
        }
        BuiltinId::BtreeRange => sorted_range(m, kind, args)?,
        _ => return Ok(None),
    }))
}

/// The ends of a `BTreeMap` or `BTreeSet`, which the sorted store keeps first and last.
fn sorted_ends(m: &Arc<Mutex<MapStore>>, kind: MapKind, id: BuiltinId) -> Value {
    match id {
        BuiltinId::FirstKeyValue | BuiltinId::LastKeyValue => {
            let store = m.lock();
            let entry = if id == BuiltinId::FirstKeyValue {
                store.first()
            } else {
                store.last()
            };
            entry.map_or_else(Value::none, |(k, v)| {
                Value::some(Value::tuple(vec![k.to_value(), v.clone()]))
            })
        }
        // `first` and `last` of a `BTreeSet`, the sorted store keeps them at the ends
        BuiltinId::First | BuiltinId::Last => {
            let store = m.lock();
            let entry = if id == BuiltinId::First {
                store.first()
            } else {
                store.last()
            };
            entry.map_or_else(Value::none, |(k, _)| Value::some(k.to_value()))
        }
        BuiltinId::PopFirst | BuiltinId::PopLast => {
            let mut store = m.lock();
            let popped = if id == BuiltinId::PopFirst {
                store.shift_remove_index(0)
            } else {
                store.pop()
            };
            match popped {
                Some((k, _)) if kind == MapKind::Set => Value::some(k.to_value()),
                Some((k, v)) => Value::some(Value::tuple(vec![k.to_value(), v])),
                None => Value::none(),
            }
        }
        _ => Value::none(),
    }
}

/// `range` on a `BTreeMap` or `BTreeSet`, lowered by the compiler to start, has start, end,
/// has end, inclusive. The entries are copied out, so the iterator does not hold the lock.
fn sorted_range(m: &Arc<Mutex<MapStore>>, kind: MapKind, args: &[Value]) -> Result<Value> {
    let flag = |i: usize| matches!(args.get(i), Some(Value::Bool(true)));
    let bound = |i: usize| -> Result<Option<MapKey>> {
        if !flag(i + 1) {
            return Ok(None);
        }
        let v = args
            .get(i)
            .ok_or_else(|| anyhow!("range needs its bounds"))?;
        v.as_key()
            .map(Some)
            .ok_or_else(|| anyhow!("invalid range bound"))
    };
    let (start, end, inclusive) = (bound(0)?, bound(2)?, flag(4));
    // the range owns its bounds and the call drops it, the start before the end
    for (key, slot) in [(&start, 0), (&end, 2)] {
        if key.as_ref().is_some_and(MapKey::may_drop)
            && let Some(value) = args.get(slot)
        {
            discard(value.clone());
        }
    }
    let store = m.lock();
    // the std messages, std checks the bounds only when the map has entries
    if let (Some(s), Some(e)) = (&start, &end)
        && !store.is_empty()
    {
        if s > e {
            bail!("range start is greater than range end in BTreeMap");
        }
        if s == e && !inclusive {
            bail!("range start and end are equal and excluded in BTreeMap");
        }
    }
    let from = start.map_or(0, |s| store.lower_bound(&s, true));
    let to = end.map_or(store.len(), |e| store.lower_bound(&e, !inclusive));
    let items: Vec<Value> = store
        .get_range(from..to.max(from))
        .into_iter()
        .flat_map(|slice| slice.iter())
        .map(|(k, v)| {
            if kind == MapKind::Set {
                k.to_value()
            } else {
                Value::tuple(vec![k.to_value(), v.clone()])
            }
        })
        .collect();
    Ok(iterator::value_iter(Arc::new(Mutex::new(items))))
}

fn set_items(m: &Arc<Mutex<MapStore>>) -> Value {
    Value::vec(m.lock().keys().map(MapKey::to_value).collect())
}

pub(super) fn map_pairs(m: &Arc<Mutex<MapStore>>) -> Value {
    Value::vec(
        m.lock()
            .iter()
            .map(|(k, v)| Value::tuple(vec![k.to_value(), v.clone()]))
            .collect(),
    )
}

/// `into_keys` and `into_values` consume the map, so its entries move into an iterator that
/// owns and drops them.
fn map_into_iterator(m: &Arc<Mutex<MapStore>>, keys: bool) -> Value {
    let store = m.lock().take_all();
    let items: Vec<Value> = if keys {
        store.keys().map(MapKey::to_value).collect()
    } else {
        store.into_values().collect()
    };
    super::iterator::owned_iterator(items)
}

/// A set or a map collected from a borrowing iterator holds references. Its keys are plain
/// copies here, so the whole collection sits behind a borrow and drops nothing of the source.
/// Only keys that can run a `Drop` need it.
fn keep_lent(collection: Value, lent: bool) -> Value {
    if lent {
        Value::Ref(Arc::new(super::value::ValueRef::borrowed(collection)))
    } else {
        collection
    }
}

/// `sorted` builds a `BTreeMap`.
pub(super) fn collect_map(items: Vec<Value>, sorted: bool) -> Result<Value> {
    collect_map_of(items, sorted, false)
}

/// `lent` says the keys are references into another collection.
pub(super) fn collect_map_of(items: Vec<Value>, sorted: bool, lent: bool) -> Result<Value> {
    let mut map = if sorted {
        MapStore::sorted()
    } else {
        MapStore::default()
    };
    let mut entries = Vec::with_capacity(items.len());
    for item in items {
        let Value::Tuple(pair) = item else {
            bail!("collect into a map needs (key, value) items");
        };
        let mut pair = pair.lock();
        if pair.len() != 2 {
            bail!("collect into a map needs (key, value) items");
        }
        let value = take(&mut pair[1]);
        let key = take(&mut pair[0])
            .into_key()
            .ok_or_else(|| anyhow!("invalid map key"))?;
        entries.push((key, value));
    }
    let lent = lent && entries.iter().any(|(key, _)| key.may_drop());
    fill(&mut map, entries, sorted, lent);
    Ok(keep_lent(Value::map_of(map), lent))
}

/// Puts collected entries into an empty store and drops what a repeated key pushes out. A
/// `BTreeMap` sorts the entries first and keeps the last of equal keys, so the earlier ones
/// drop in key order. A `HashMap` inserts one by one, it keeps the first key, drops the new
/// one and drops the value it replaces. `lent` keys are references into another collection, a
/// repeated one drops nothing.
fn fill(store: &mut MapStore, mut entries: Vec<(MapKey, Value)>, sorted: bool, lent: bool) {
    if sorted {
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let mut entries = entries.into_iter().peekable();
        while let Some((key, value)) = entries.next() {
            if entries.peek().is_some_and(|next| next.0 == key) {
                if !lent {
                    discard_key(&key);
                }
                discard(value);
            } else {
                store.insert(key, value);
            }
        }
        return;
    }
    for (key, value) in entries {
        if !lent && store.contains_key(&key) {
            discard_key(&key);
        }
        if let Some(old) = store.insert(key, value) {
            discard(old);
        }
    }
}

/// `insert` on a store that may hold the key. std keeps the key it has, drops the new one and
/// drops the value it replaces.
fn insert_or_replace(store: &mut MapStore, key: MapKey, value: Value) {
    if store.contains_key(&key) {
        discard_key(&key);
    }
    if let Some(old) = store.insert(key, value) {
        discard(old);
    }
}

/// `sorted` builds a `BTreeSet`.
pub(super) fn collect_set(items: Vec<Value>, sorted: bool) -> Result<Value> {
    collect_set_of(items, sorted, false)
}

/// `lent` says the items are references into another collection.
pub(super) fn collect_set_of(items: Vec<Value>, sorted: bool, lent: bool) -> Result<Value> {
    let mut set = if sorted {
        MapStore::sorted()
    } else {
        MapStore::default()
    };
    let mut entries = Vec::with_capacity(items.len());
    for item in items {
        let key = item.into_key().ok_or_else(|| anyhow!("invalid set key"))?;
        entries.push((key, Value::Unit));
    }
    let lent = lent && entries.iter().any(|(key, _)| key.may_drop());
    fill(&mut set, entries, sorted, lent);
    Ok(keep_lent(Value::set_of(set), lent))
}

/// A `&mut V` into the entry of `key`.
fn entry_ref(m: &Arc<Mutex<MapStore>>, key: MapKey) -> Value {
    Value::Ref(Arc::new(super::value::ValueRef::map_entry(m.clone(), key)))
}

/// The methods that write into the entries, `values_mut`, `iter_mut` and `extend`. None for any
/// other method.
fn map_write_method(
    m: &Arc<Mutex<MapStore>>,
    kind: MapKind,
    method: &MethodName,
    args: &[Value],
) -> Result<Option<Value>> {
    Ok(Some(match method.id {
        // `&mut V` items, so a write through one lands in the entry
        BuiltinId::ValuesMut => {
            let keys: Vec<MapKey> = m.lock().keys().cloned().collect();
            Value::vec(keys.into_iter().map(|k| entry_ref(m, k)).collect())
        }
        BuiltinId::IterMut if kind != MapKind::Set => {
            let keys: Vec<MapKey> = m.lock().keys().cloned().collect();
            Value::vec(
                keys.into_iter()
                    .map(|k| Value::tuple(vec![k.to_value(), entry_ref(m, k)]))
                    .collect(),
            )
        }
        BuiltinId::Extend => {
            let Some(Value::Vec(items)) = args.first() else {
                bail!("`extend` needs something iterable");
            };
            let items = take(&mut *items.lock());
            let mut store = m.lock();
            for item in items {
                let (k, v) = if kind == MapKind::Set {
                    (item, Value::Unit)
                } else {
                    let Value::Tuple(pair) = item else {
                        bail!("a map `extend` needs key and value pairs");
                    };
                    let mut pair = take(&mut *pair.lock()).into_iter();
                    let (Some(k), Some(v)) = (pair.next(), pair.next()) else {
                        bail!("a map `extend` needs key and value pairs");
                    };
                    (k, v)
                };
                let k = k.into_key().ok_or_else(|| anyhow!("invalid map key"))?;
                insert_or_replace(&mut store, k, v);
            }
            Value::Unit
        }
        _ => return Ok(None),
    }))
}
