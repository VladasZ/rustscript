//! Builtin methods on `Vec`, `HashMap` and `HashSet`.

use num_traits::AsPrimitive;
use std::cmp::Ordering;
use std::mem::take;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use parking_lot::Mutex;

use super::bridge::arg;
use super::bytecode::{BuiltinId, MethodName, ScalarTy};
use super::enum_def::EnumKind;
use super::iterator;
use super::native::Native;
use super::ops::compare_values;
use super::value::{List, MapKey, MapKind, Value};

pub(super) use super::value::MapStore;

pub(super) fn vec_method(v: &List, method: &MethodName, args: &mut [Value]) -> Result<Value> {
    if let Some(out) = deque_method(v, method.id, args)? {
        return Ok(out);
    }
    Ok(match method.id {
        BuiltinId::Len | BuiltinId::Count => super::shared::usize_value(v.lock().len()),
        BuiltinId::IsEmpty => Value::Bool(v.lock().is_empty()),
        BuiltinId::Clone => Value::Vec(v.clone()).deep_clone(),
        BuiltinId::Iter | BuiltinId::IntoIter => iterator::value_iter(v.clone()),
        BuiltinId::IterMut => iterator::value_iter_mut(v.clone()),
        BuiltinId::Push | BuiltinId::PushBack => {
            v.lock().push(args.first_mut().map_or(Value::Unit, take));
            Value::Unit
        }
        BuiltinId::Pop | BuiltinId::PopBack => match v.lock().pop() {
            Some(x) => Value::some(x),
            None => Value::none(),
        },
        BuiltinId::Insert => {
            let i = usize::try_from(int_arg(args, 0)?)?;
            v.lock().insert(i, arg(args, 1)?);
            Value::Unit
        }
        BuiltinId::Remove => {
            let i = usize::try_from(int_arg(args, 0)?)?;
            let mut items = v.lock();
            if i >= items.len() {
                bail!(
                    "removal index (is {i}) should be < len (is {})",
                    items.len()
                );
            }
            items.remove(i)
        }
        BuiltinId::Get | BuiltinId::GetMut => vec_get(v, method, args),
        BuiltinId::FirstMut | BuiltinId::FrontMut => edge_element_ref(v, true),
        BuiltinId::LastMut | BuiltinId::BackMut => edge_element_ref(v, false),
        BuiltinId::First | BuiltinId::Front => v
            .lock()
            .first()
            .cloned()
            .map_or_else(Value::none, Value::some),
        BuiltinId::Last | BuiltinId::Back | BuiltinId::NextBack => v
            .lock()
            .last()
            .cloned()
            .map_or_else(Value::none, Value::some),
        BuiltinId::SplitFirst => match v.lock().split_first() {
            Some((head, rest)) => {
                Value::some(Value::tuple(vec![head.clone(), Value::vec(rest.to_vec())]))
            }
            None => Value::none(),
        },
        BuiltinId::Contains => {
            let needle = arg(args, 0)?;
            Value::Bool(v.lock().iter().any(|x| x.eq_value(&needle)))
        }
        BuiltinId::Sort | BuiltinId::SortUnstable => {
            let mut items = v.lock();
            items.sort_by_key(sort_key);
            Value::Unit
        }
        BuiltinId::Join => vec_join(v, args),
        BuiltinId::Concat => vec_concat(v, method.scalar.as_ref()),
        BuiltinId::Sum => return vec_sum(v, method),
        BuiltinId::Product => return vec_product(v),
        BuiltinId::Rev => {
            let mut items = v.lock().clone();
            items.reverse();
            Value::vec(items)
        }
        BuiltinId::Enumerate => Value::vec(
            v.lock()
                .iter()
                .enumerate()
                .map(|(i, x)| {
                    Value::tuple(vec![Value::Int(super::shared::usize_i64(i)), x.clone()])
                })
                .collect(),
        ),
        BuiltinId::Take => {
            let n = usize::try_from(int_arg(args, 0)?)?;
            Value::vec(v.lock().iter().take(n).cloned().collect())
        }
        BuiltinId::Skip => {
            let n = usize::try_from(int_arg(args, 0)?)?;
            Value::vec(v.lock().iter().skip(n).cloned().collect())
        }
        _ => return vec_method_by_name(v, method, args),
    })
}

/// `get_mut` gives a real element reference so writes land. A non integer argument is None like
/// in serde.
fn vec_get(v: &List, method: &MethodName, args: &[Value]) -> Value {
    let index = args
        .first()
        .and_then(Value::int_parts)
        .and_then(|(index, _)| usize::try_from(index).ok());
    let Some(i) = index else {
        return Value::none();
    };
    if method.id == BuiltinId::GetMut {
        return if i < v.lock().len() {
            Value::some(Value::Ref(Arc::new(super::value::ValueRef::vec_element(
                v.clone(),
                i,
            ))))
        } else {
            Value::none()
        };
    }
    match v.lock().get(i).cloned() {
        Some(x) => Value::some(x),
        None => Value::none(),
    }
}

/// Real element references, so writes land in the vec.
/// The `VecDeque` front end and the search, a `VecDeque` is a `Vec` here.
fn deque_method(v: &List, id: BuiltinId, args: &mut [Value]) -> Result<Option<Value>> {
    Ok(Some(match id {
        BuiltinId::PushFront => {
            v.lock()
                .insert(0, args.first_mut().map_or(Value::Unit, take));
            Value::Unit
        }
        BuiltinId::PopFront => {
            let mut items = v.lock();
            if items.is_empty() {
                Value::none()
            } else {
                Value::some(items.remove(0))
            }
        }
        // the slice is the storage itself
        BuiltinId::MakeContiguous => Value::Vec(v.clone()),
        BuiltinId::BinarySearch => binary_search(&v.lock(), &arg(args, 0)?)?,
        _ => return Ok(None),
    }))
}

/// `Ok(index)` of a match, `Err(index)` where it would go. Elements are compared with the value
/// order, so a mixed list reports its error like a comparison would.
fn binary_search(items: &[Value], needle: &Value) -> Result<Value> {
    let (mut lo, mut hi) = (0usize, items.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match compare_values(&items[mid], needle)? {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Ok(Value::ok(super::shared::usize_value(mid))),
        }
    }
    Ok(Value::err(super::shared::usize_value(lo)))
}

fn edge_element_ref(v: &List, first: bool) -> Value {
    let len = v.lock().len();
    if len == 0 {
        return Value::none();
    }
    let index = if first { 0 } else { len - 1 };
    Value::some(Value::Ref(Arc::new(super::value::ValueRef::vec_element(
        v.clone(),
        index,
    ))))
}

fn vec_sum(v: &List, method: &MethodName) -> Result<Value> {
    iterator::sum_values(v.lock().clone(), method.scalar.as_ref())
}

/// Floats fold in at the end.
fn vec_product(v: &List) -> Result<Value> {
    Ok({
        let mut acc_i = 1i64;
        let mut acc_f = 1f64;
        let mut is_float = false;
        for x in v.lock().iter() {
            match &x.bridge_image().unwrap_or_else(|| x.clone()) {
                Value::Int(i) => {
                    acc_i = acc_i
                        .checked_mul(*i)
                        .ok_or_else(|| anyhow!("attempt to multiply with overflow"))?;
                }
                Value::Float(f) => {
                    is_float = true;
                    acc_f *= f;
                }
                _ => bail!("product needs numbers"),
            }
        }
        if is_float {
            Value::Float(acc_f * AsPrimitive::<f64>::as_(acc_i))
        } else {
            Value::Int(acc_i)
        }
    })
}

/// Nested vecs flatten, strings join, told apart by the first element. An empty receiver goes by
/// the written element type, or the string form.
fn vec_concat(v: &List, element: Option<&ScalarTy>) -> Value {
    let items = v.lock();
    if items.is_empty() && matches!(element, Some(ScalarTy::List(_))) {
        return Value::vec(Vec::new());
    }
    match items.first() {
        Some(Value::Vec(_)) => {
            let mut out = Vec::new();
            for x in items.iter() {
                if let Value::Vec(inner) = x {
                    out.extend(inner.lock().iter().cloned());
                }
            }
            Value::vec(out)
        }
        _ => Value::str(items.iter().map(Value::display).collect::<String>()),
    }
}

fn vec_join(v: &List, args: &[Value]) -> Value {
    let sep = args.first().map(Value::display).unwrap_or_default();
    let joined = v
        .lock()
        .iter()
        .map(Value::display)
        .collect::<Vec<_>>()
        .join(&sep);
    Value::str(joined)
}

/// Everything without a builtin id.
fn vec_method_by_name(v: &List, method: &MethodName, args: &mut [Value]) -> Result<Value> {
    Ok(match method.id {
        BuiltinId::ToVec | BuiltinId::Collect | BuiltinId::Cloned | BuiltinId::Copied => {
            Value::Vec(v.clone()).deep_clone()
        }
        // `by_ref` is a draining view over the same vector, so whatever it hands on is gone from
        // this one too
        BuiltinId::ByRef => iterator::draining_iter(v.clone()),
        BuiltinId::Peekable => iterator::peekable_draining(v.clone()),
        BuiltinId::Nth => match v.lock().get(usize::try_from(int_arg(args, 0)?)?) {
            Some(item) => Value::some(item.clone()),
            None => Value::none(),
        },
        BuiltinId::AsSlice
        | BuiltinId::Windows
        | BuiltinId::Chunks
        | BuiltinId::Repeat
        | BuiltinId::Swap => return vec_slice_view(v, method.id, args),
        BuiltinId::CollectString => {
            Value::str(v.lock().iter().map(Value::display).collect::<String>())
        }
        BuiltinId::CollectMap => return collect_map(v.lock().clone()),
        BuiltinId::CollectSet => return collect_set(v.lock().clone()),
        BuiltinId::Reverse => {
            v.lock().reverse();
            Value::Unit
        }
        BuiltinId::Dedup => {
            let mut items = v.lock();
            items.dedup_by(|a, b| a.eq_value(b));
            Value::Unit
        }
        BuiltinId::Clear => {
            v.lock().clear();
            Value::Unit
        }
        BuiltinId::CopyFromSlice => return vec_copy_from_slice(v, args),
        BuiltinId::SwapRemove => {
            let i = usize::try_from(int_arg(args, 0)?)?;
            let mut items = v.lock();
            if i >= items.len() {
                bail!(
                    "swap_remove index (is {i}) should be < len (is {})",
                    items.len()
                );
            }
            items.swap_remove(i)
        }
        BuiltinId::Truncate => {
            let n = usize::try_from(int_arg(args, 0)?)?;
            v.lock().truncate(n);
            Value::Unit
        }
        // A lazy argument is drained in `eval_method` first. Anything else is an error, a silent
        // no-op would hide the bug.
        BuiltinId::Extend | BuiltinId::Append | BuiltinId::ExtendFromSlice => {
            let Some(Value::Vec(other)) = args.first() else {
                bail!("`{}` needs something iterable", method.text);
            };
            // cloned first, so extending a vec with itself doesn't deadlock
            let appended: Vec<Value> = other.lock().clone();
            v.lock().extend(appended);
            Value::Unit
        }
        // 1 level, `Ok` and `Some` yield their inner value, `Err` and `None` drop out
        BuiltinId::Flatten => {
            let items = v.lock().clone();
            let mut out: Vec<Value> = Vec::new();
            for item in &items {
                match item {
                    Value::Vec(inner) => out.extend(inner.lock().iter().cloned()),
                    Value::Enum { def, .. }
                        if matches!(def.kind, EnumKind::Option | EnumKind::Result) =>
                    {
                        if let Some(inner) = item.success_payload() {
                            out.push(inner);
                        }
                    }
                    other => out.push(other.clone()),
                }
            }
            Value::vec(out)
        }
        // `next` takes the front item, handing it back without removing it makes a following
        // `collect` see it again
        BuiltinId::Next => {
            let mut items = v.lock();
            if items.is_empty() {
                Value::none()
            } else {
                Value::some(items.remove(0))
            }
        }
        BuiltinId::Max | BuiltinId::Min => return vec_min_max(v, method, args),
        // a parsed json array is a plain Vec
        BuiltinId::AsArray => Value::some(Value::vec(v.lock().clone())),
        // the mut accessor hands back the same list, so a push reaches the original
        BuiltinId::AsArrayMut => Value::some(Value::Ref(Arc::new(
            super::value::ValueRef::borrowed(Value::Vec(v.clone())),
        ))),
        BuiltinId::AsObject | BuiltinId::AsObjectMut => Value::none(),
        // any receiver names live in 1 place
        _ => {
            return super::methods::generic_method(&Value::Vec(v.clone()), method, args);
        }
    })
}

/// `v[a..b].copy_from_slice(src)` with the bounds as leading args, so the write reaches the base
/// vec. An open end arrives as the max sentinel.
fn vec_copy_from_slice(v: &List, args: &[Value]) -> Result<Value> {
    let start = usize::try_from(int_arg(args, 0)?)?;
    let end_raw = int_arg(args, 1)?;
    let src: Vec<Value> = match args.get(2) {
        Some(Value::Vec(other)) => other.lock().clone(),
        _ => bail!("copy_from_slice takes a slice argument"),
    };
    let mut items = v.lock();
    let end = if end_raw == i64::MAX {
        items.len()
    } else {
        usize::try_from(end_raw)?
    };
    if end > items.len() {
        bail!(
            "range end index {end} out of range for slice of length {}",
            items.len()
        );
    }
    let dst_len = end.saturating_sub(start);
    if dst_len != src.len() {
        bail!(
            "source slice length ({}) does not match destination slice length ({dst_len})",
            src.len()
        );
    }
    for (k, val) in src.into_iter().enumerate() {
        items[start + k] = val;
    }
    Ok(Value::Unit)
}

/// With an argument this is `Ord::max` on 2 whole vecs, without one the iterator reduction.
fn vec_min_max(v: &List, method: &MethodName, args: &[Value]) -> Result<Value> {
    if let Some(other) = args.first() {
        let recv = Value::Vec(v.clone());
        let ord = compare_values(&recv, other)?;
        let take_recv = if method.id == BuiltinId::Max {
            ord.is_ge()
        } else {
            ord.is_le()
        };
        return Ok(if take_recv { recv } else { other.clone() });
    }
    let items = v.lock().clone();
    let mut best: Option<&Value> = None;
    for item in &items {
        let better = match best {
            Some(b) => {
                let ord = compare_values(item, b)?;
                if method.id == BuiltinId::Max {
                    ord.is_gt()
                } else {
                    ord.is_lt()
                }
            }
            None => true,
        };
        if better {
            best = Some(item);
        }
    }
    Ok(best.cloned().map_or_else(Value::none, Value::some))
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
    Ok(match method.id {
        BuiltinId::Len | BuiltinId::Count => super::shared::usize_value(m.lock().len()),
        BuiltinId::IsEmpty => Value::Bool(m.lock().is_empty()),
        BuiltinId::Clone => Value::Map(m.clone(), kind).deep_clone(),
        BuiltinId::Insert => {
            let k = take(&mut args[0])
                .into_key()
                .ok_or_else(|| anyhow!("invalid map key"))?;
            // a set insert returns whether it was new, a map insert the old value
            if kind == MapKind::Set {
                let old = m.lock().insert(k, Value::Unit);
                return Ok(Value::Bool(old.is_none()));
            }
            let val = args.get_mut(1).map_or(Value::Unit, take);
            let old = m.lock().insert(k, val);
            match old {
                Some(v) => Value::some(v),
                None => Value::none(),
            }
        }
        // a set `get` returns the element, not the Unit that backs it
        BuiltinId::Get if kind == MapKind::Set => {
            let arg = args.first().ok_or_else(|| anyhow!("invalid map key"))?;
            let k = arg.as_key().ok_or_else(|| anyhow!("invalid map key"))?;
            match m.lock().get_key_value(&k) {
                Some((key, _)) => Value::some(key.to_value()),
                None => Value::none(),
            }
        }
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
        BuiltinId::Contains if kind == MapKind::Set => lookup(0, &|v| Value::Bool(v.is_some()))?,
        BuiltinId::IsSubset
        | BuiltinId::IsSuperset
        | BuiltinId::IsDisjoint
        | BuiltinId::Union
        | BuiltinId::Intersection
        | BuiltinId::Difference
        | BuiltinId::SymmetricDifference
            if kind == MapKind::Set =>
        {
            return set_relation(m, method.id, args);
        }
        BuiltinId::ContainsKey => lookup(0, &|v| Value::Bool(v.is_some()))?,
        BuiltinId::Remove => {
            let arg = args.first().ok_or_else(|| anyhow!("invalid map key"))?;
            let k = arg.as_key().ok_or_else(|| anyhow!("invalid map key"))?;
            let removed = m.lock().shift_remove(&k);
            if kind == MapKind::Set {
                return Ok(Value::Bool(removed.is_some()));
            }
            match removed {
                Some(v) => Value::some(v),
                None => Value::none(),
            }
        }
        BuiltinId::Keys | BuiltinId::IntoKeys => {
            Value::vec(m.lock().keys().map(MapKey::to_value).collect())
        }
        BuiltinId::Values | BuiltinId::IntoValues | BuiltinId::ValuesMut => {
            Value::vec(m.lock().values().cloned().collect())
        }
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
        BuiltinId::Iter | BuiltinId::IntoIter | BuiltinId::Drain if kind == MapKind::Set => {
            set_items(m)
        }
        BuiltinId::Iter | BuiltinId::IntoIter | BuiltinId::Drain => map_pairs(m),
        // a parsed json object is an Arc shared Map, so the mut accessor hands back the same map
        BuiltinId::AsObject => Value::some(Value::Map(m.clone(), kind)),
        BuiltinId::AsObjectMut => Value::some(Value::Ref(Arc::new(
            super::value::ValueRef::borrowed(Value::Map(m.clone(), kind)),
        ))),
        BuiltinId::AsArray | BuiltinId::AsArrayMut => Value::none(),
        _ => return super::methods::generic_method(&Value::Map(m.clone(), kind), method, args),
    })
}

fn vec_slice_view(v: &List, id: BuiltinId, args: &[Value]) -> Result<Value> {
    Ok(match id {
        // the value model has no separate slice type
        BuiltinId::AsSlice => Value::Vec(v.clone()),
        BuiltinId::Windows => {
            let size = usize::try_from(int_arg(args, 0)?)?;
            if size == 0 {
                bail!("window size must be non-zero");
            }
            let items = v.lock();
            iterator::value_iter(Arc::new(Mutex::new(
                items
                    .windows(size)
                    .map(|w| Value::vec(w.to_vec()))
                    .collect(),
            )))
        }
        BuiltinId::Chunks => {
            let size = usize::try_from(int_arg(args, 0)?)?;
            if size == 0 {
                bail!("chunk size must be non-zero");
            }
            let items = v.lock();
            iterator::value_iter(Arc::new(Mutex::new(
                items.chunks(size).map(|c| Value::vec(c.to_vec())).collect(),
            )))
        }
        BuiltinId::Repeat => {
            // a count past `usize` is a huge count, not a conversion failure
            let n = usize::try_from(int_arg(args, 0)?).unwrap_or(usize::MAX);
            let items = v.lock();
            // A script panic, not an interpreter death with another exit code. The line is
            // `isize::MAX` bytes, so elements are weighed like the allocator does.
            let total = items.len().saturating_mul(n);
            let bytes = total.saturating_mul(size_of::<Value>());
            if bytes > isize::MAX.cast_unsigned() {
                bail!("capacity overflow");
            }
            let mut out = Vec::with_capacity(total);
            // repeating nothing is nothing, the loop would run for the whole count
            if !items.is_empty() {
                for _ in 0..n {
                    out.extend(items.iter().cloned());
                }
            }
            Value::vec(out)
        }
        BuiltinId::Swap => {
            let a = usize::try_from(int_arg(args, 0)?)?;
            let b = usize::try_from(int_arg(args, 1)?)?;
            let mut items = v.lock();
            let len = items.len();
            for i in [a, b] {
                if i >= len {
                    bail!("index out of bounds: the len is {len} but the index is {i}");
                }
            }
            items.swap(a, b);
            Value::Unit
        }
        _ => unreachable!("vec_slice_view handles the slice views only"),
    })
}

/// The combinations iterate this set's elements then the other's. Real Rust doesn't promise any
/// order here.
fn set_relation(m: &Arc<Mutex<MapStore>>, id: BuiltinId, args: &[Value]) -> Result<Value> {
    let Some(Value::Map(other, MapKind::Set)) = args.first() else {
        bail!("set operation needs a set argument");
    };
    // snapshots, a set compared with itself would relock
    let mine: Vec<MapKey> = m.lock().keys().cloned().collect();
    let theirs: MapStore = other.lock().clone();
    let has = |k: &MapKey| theirs.contains_key(k);
    let elems = |keys: Vec<MapKey>| {
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

pub(super) fn collect_map(items: Vec<Value>) -> Result<Value> {
    let mut map = MapStore::default();
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
        map.insert(key, value);
    }
    Ok(Value::map_of(map))
}

pub(super) fn collect_set(items: Vec<Value>) -> Result<Value> {
    let mut set = MapStore::default();
    for item in items {
        let key = item.into_key().ok_or_else(|| anyhow!("invalid set key"))?;
        set.insert(key, Value::Unit);
    }
    Ok(Value::set_of(set))
}

pub(super) fn int_arg(args: &[Value], i: usize) -> Result<i64> {
    match args.get(i).and_then(Value::int_parts) {
        // a count past i64 saturates like the old i64 image
        Some((n, _)) => Ok(i64::try_from(n).unwrap_or(i64::MAX)),
        None => bail!("expected an integer argument"),
    }
}

/// Good enough for numbers and strings.
pub(super) fn sort_key(v: &Value) -> SortKey {
    match v {
        Value::Int(i) => SortKey::Int(i128::from(*i)),
        // the full i128 value, so 2 u64 values past `i64::MAX` still order
        Value::IntW(..) => match v.int_parts() {
            Some((i, _)) => SortKey::Int(i),
            None => SortKey::Str(v.display()),
        },
        Value::F32(f) => SortKey::Float(f64::from(*f)),
        Value::Float(f) => SortKey::Float(*f),
        Value::Bool(b) => SortKey::Int(i128::from(*b)),
        Value::Str(s) => SortKey::Str(s.to_string()),
        Value::Char(c) => SortKey::Str(c.to_string()),
        Value::Tuple(items) | Value::Vec(items) => {
            SortKey::List(items.lock().iter().map(sort_key).collect())
        }
        // derived `Ord` orders by variant first, then payload, a struct by its fields in
        // declaration order
        Value::Enum { variant, data, .. } => {
            let mut keys = vec![SortKey::Int(i128::from(*variant))];
            keys.extend(data.lock().iter().map(sort_key));
            SortKey::List(keys)
        }
        Value::Struct(s) => SortKey::List(s.values.lock().iter().map(sort_key).collect()),
        other => SortKey::Str(other.display()),
    }
}

#[derive(PartialEq)]
pub(super) enum SortKey {
    Int(i128),
    Float(f64),
    Str(String),
    List(Vec<SortKey>),
}

impl Eq for SortKey {}

impl PartialOrd for SortKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SortKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (self, other) {
            (SortKey::Int(a), SortKey::Int(b)) => a.cmp(b),
            (SortKey::Float(a), SortKey::Float(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
            (SortKey::Int(a), SortKey::Float(b)) => AsPrimitive::<f64>::as_(*a)
                .partial_cmp(b)
                .unwrap_or(Ordering::Equal),
            (SortKey::Float(a), SortKey::Int(b)) => a
                .partial_cmp(&AsPrimitive::<f64>::as_(*b))
                .unwrap_or(Ordering::Equal),
            (SortKey::Str(a), SortKey::Str(b)) => a.cmp(b),
            (SortKey::List(a), SortKey::List(b)) => a.cmp(b),
            (SortKey::Int(_) | SortKey::Float(_), _) | (SortKey::Str(_), SortKey::List(_)) => {
                Ordering::Less
            }
            (_, SortKey::Int(_) | SortKey::Float(_)) | (SortKey::List(_), SortKey::Str(_)) => {
                Ordering::Greater
            }
        }
    }
}
