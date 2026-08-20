//! Builtin methods on Vec and `HashMap`/`HashSet`,
//! backed by the shared `Arc` value model.

use num_traits::AsPrimitive;
use std::mem::take;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use indexmap::IndexMap;
use parking_lot::Mutex;

use super::bytecode::{BuiltinId, MethodName};
use super::enum_def::EnumKind;
use super::iterator;
use super::native::Native;
use super::ops::compare_values;
use super::value::{List, MapKey, MapKind, Value};

pub(super) type MapStore = IndexMap<MapKey, Value>;

pub(super) fn vec_method(v: &List, method: &MethodName, args: &mut [Value]) -> Result<Value> {
    Ok(match method.id {
        BuiltinId::Len | BuiltinId::Count => super::shared::usize_value(v.lock().len()),
        BuiltinId::IsEmpty => Value::Bool(v.lock().is_empty()),
        BuiltinId::Clone => Value::vec(v.lock().clone()),
        BuiltinId::Iter | BuiltinId::IntoIter => iterator::value_iter(v.clone()),
        BuiltinId::IterMut => iterator::value_iter_mut(v.clone()),
        BuiltinId::Push => {
            v.lock().push(args.first_mut().map_or(Value::Unit, take));
            Value::Unit
        }
        BuiltinId::Pop => match v.lock().pop() {
            Some(x) => Value::some(x),
            None => Value::none(),
        },
        BuiltinId::Insert => {
            let i = usize::try_from(int_arg(args, 0)?)?;
            v.lock()
                .insert(i, args.get(1).cloned().unwrap_or(Value::Unit));
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
        BuiltinId::FirstMut => edge_element_ref(v, true),
        BuiltinId::LastMut => edge_element_ref(v, false),
        BuiltinId::First => v
            .lock()
            .first()
            .cloned()
            .map_or_else(Value::none, Value::some),
        BuiltinId::Last => v
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
            let needle = args.first().cloned().unwrap_or(Value::Unit);
            Value::Bool(v.lock().iter().any(|x| x.eq_value(&needle)))
        }
        BuiltinId::Sort | BuiltinId::SortUnstable => {
            let mut items = v.lock();
            items.sort_by_key(sort_key);
            Value::Unit
        }
        BuiltinId::Join => vec_join(v, args),
        BuiltinId::Concat => vec_concat(v),
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

/// `get` clones the element out; `get_mut` answers a real element
/// reference, `&mut V` in real Rust, so writes through it land in the
/// element. A json array reads by index, and serde answers None for a key
/// that is not one rather than failing, so a non-integer argument is None.
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

/// `first_mut` and `last_mut` answer real element references, so writes
/// through them land in the vec.
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

/// `sum` over the elements, with the -0.0 float identity and the width
/// checks the arms explain.
fn vec_sum(v: &List, method: &MethodName) -> Result<Value> {
    iterator::sum_values(v.lock().clone(), method.scalar.as_ref())
}

/// `product` over the elements, floats fold in at the end.
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

/// A vec of vecs flattens like the real slice `concat`; anything else
/// concatenates the display forms, which covers `Vec<String>`. The empty
/// case cannot know its element type, so it is a string.
fn vec_concat(v: &List) -> Value {
    let items = v.lock();
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

/// `join` through the display forms with a display-form separator.
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

/// The by-name tail of `vec_method`, everything without a builtin id.
fn vec_method_by_name(v: &List, method: &MethodName, args: &mut [Value]) -> Result<Value> {
    Ok(match method.id {
        BuiltinId::ToVec | BuiltinId::Collect | BuiltinId::Cloned | BuiltinId::Copied => {
            Value::vec(v.lock().clone())
        }
        // `by_ref` lends the iterator out, so whatever the borrow hands on
        // is gone from this one too. A draining view over the same vector
        // is that borrow.
        BuiltinId::ByRef => iterator::draining_iter(v.clone()),
        BuiltinId::Peekable => iterator::peekable_draining(v.clone()),
        BuiltinId::Nth => match v.lock().get(usize::try_from(int_arg(args, 0)?)?) {
            Some(item) => Value::some(item.clone()),
            None => Value::none(),
        },
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
        // A lazy iterator argument is drained into a vec before it gets
        // here, see `eval_method`'s extend pre-pass. Anything else that is
        // not a vec is an error rather than a silent no-op: extending by
        // nothing and reporting success hides the bug in the caller's data.
        BuiltinId::Extend | BuiltinId::Append | BuiltinId::ExtendFromSlice => {
            let Some(Value::Vec(other)) = args.first() else {
                bail!("`{}` needs something iterable", method.text);
            };
            // Cloned before the extend, so extending a vec with itself
            // does not deadlock on the same mutex.
            let appended: Vec<Value> = other.lock().clone();
            v.lock().extend(appended);
            Value::Unit
        }
        // Flattens one level: nested vectors spill their items, and Ok/Some
        // yield their inner value while Err/None drop out.
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
        // Iterators are eager vectors here, so `next` takes the front item
        // and leaves the rest behind. Handing back the first item without
        // removing it left the iterator unconsumed, so a following `collect`
        // saw the item again.
        BuiltinId::Next => {
            let mut items = v.lock();
            if items.is_empty() {
                Value::none()
            } else {
                Value::some(items.remove(0))
            }
        }
        BuiltinId::Max | BuiltinId::Min => return vec_min_max(v, method, args),
        // A JSON array parsed by the interpreter is a plain Vec, so the
        // serde_json accessors resolve against it here.
        BuiltinId::AsArray => Value::some(Value::vec(v.lock().clone())),
        // The mut accessor has to hand back the same list, not a copy,
        // so a push through it reaches the value it was taken from.
        BuiltinId::AsArrayMut => Value::some(Value::Ref(Arc::new(
            super::value::ValueRef::borrowed(Value::Vec(v.clone())),
        ))),
        BuiltinId::AsObject | BuiltinId::AsObjectMut => Value::none(),
        // Names that apply to any receiver, `clone` and `into` and the
        // rest, live in one place instead of being repeated per type.
        _ => {
            return super::methods::generic_method(&Value::Vec(v.clone()), method, args);
        }
    })
}

/// Compiled from `v[a..b].copy_from_slice(src)` with the bounds as leading
/// args, so the write reaches the base vec instead of a copied slice
/// temporary. An open end arrives as the max sentinel.
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

/// With an argument this is `Ord::max` on two whole vecs, which orders them
/// lexicographically and hands one back. Only the no-argument form is the
/// iterator reduction over the elements.
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
        BuiltinId::Clone => Value::Map(Arc::new(Mutex::new(m.lock().clone())), kind),
        BuiltinId::Insert => {
            let k = take(&mut args[0])
                .into_key()
                .ok_or_else(|| anyhow!("invalid map key"))?;
            // A set's insert takes only the element and answers whether it
            // was new, a map's takes a value and answers the old one.
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
        // A set's get answers the stored element itself, not the Unit value
        // that backs it.
        BuiltinId::Get if kind == MapKind::Set => {
            let arg = args.first().ok_or_else(|| anyhow!("invalid map key"))?;
            let k = arg.as_key().ok_or_else(|| anyhow!("invalid map key"))?;
            match m.lock().get_key_value(&k) {
                Some((key, _)) => Value::some(key.to_value()),
                None => Value::none(),
            }
        }
        // `get_mut` answers `&mut V` in real Rust, so writes through the
        // answer must land in the entry. A clone would drop them.
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
        BuiltinId::ContainsKey => lookup(0, &|v| Value::Bool(v.is_some()))?,
        BuiltinId::Remove => {
            let arg = args.first().ok_or_else(|| anyhow!("invalid map key"))?;
            let k = arg.as_key().ok_or_else(|| anyhow!("invalid map key"))?;
            let removed = m.lock().shift_remove(&k);
            // A set's remove answers whether the element was there.
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
        BuiltinId::Values | BuiltinId::IntoValues => {
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
        BuiltinId::Iter | BuiltinId::IntoIter if kind == MapKind::Set => set_items(m),
        BuiltinId::Iter | BuiltinId::IntoIter => map_pairs(m),
        _ => match method.id {
            BuiltinId::ValuesMut => Value::vec(m.lock().values().cloned().collect()),
            BuiltinId::Drain if kind == MapKind::Set => set_items(m),
            BuiltinId::Drain => map_pairs(m),
            // A JSON object parsed by the interpreter is a Map, and it is Arc
            // shared, so the mut accessor is the same call: what it hands back
            // is the same map, and an insert through it reaches the original.
            BuiltinId::AsObject => Value::some(Value::Map(m.clone(), kind)),
            BuiltinId::AsObjectMut => Value::some(Value::Ref(Arc::new(
                super::value::ValueRef::borrowed(Value::Map(m.clone(), kind)),
            ))),
            BuiltinId::AsArray | BuiltinId::AsArrayMut => Value::none(),
            _ => {
                return super::methods::generic_method(&Value::Map(m.clone(), kind), method, args);
            }
        },
    })
}

/// The elements of a set, for `iter` and `into_iter` and `drain`.
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

/// `collect` into a `HashMap`, from `(key, value)` items.
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

/// `collect` into a `HashSet`.
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
        // A count past i64 saturates, matching the i64 image such an
        // argument passed before widths were real.
        Some((n, _)) => Ok(i64::try_from(n).unwrap_or(i64::MAX)),
        None => bail!("expected an integer argument"),
    }
}

/// Ordering key for `sort`, good enough for numbers and strings.
pub(super) fn sort_key(v: &Value) -> SortKey {
    match v {
        Value::Int(i) => SortKey::Int(i128::from(*i)),
        // The full i128 value, so two u64 values past i64::MAX still order
        // by value rather than tying at a clamp.
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
