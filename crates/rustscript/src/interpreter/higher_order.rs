//! The closure taking methods on Vec, map entries, Option, Result and lazy iterators.

use std::slice::from_ref;
use std::sync::Arc;

use anyhow::Result;

use super::bridge::arg;
use super::bytecode::BuiltinId;
use super::enum_def::{EQUAL, EnumKind, OK, SOME};
use super::iterator::{as_closure, option_inner};
use super::methods::ordering_from_value;
use super::native::Native;
use super::shared::usize_i64;
use super::value::{List, Map, MapKey, Value, ValueRef};
use super::vecmap::{SortKey, sort_key};
use super::vm::Vm;

impl Vm {
    /// None when the method is not one of these.
    pub(super) fn higher_order(
        self: &Arc<Self>,
        recv: &Value,
        name: BuiltinId,
        args: &[Value],
    ) -> Result<Option<Value>> {
        match recv {
            // `then` takes a closure, unlike `then_some`
            Value::Bool(b) if name == BuiltinId::Then => {
                if !*b {
                    return Ok(Some(Value::none()));
                }
                let f = as_closure(args.first())?;
                Ok(Some(Value::some(self.call_closure_data(&f, &[])?)))
            }
            Value::Enum { def, variant, .. }
                if def.kind == EnumKind::Ordering && name == BuiltinId::ThenWith =>
            {
                if *variant == EQUAL {
                    let f = as_closure(args.first())?;
                    Ok(Some(self.call_closure_data(&f, &[])?))
                } else {
                    Ok(Some(recv.clone()))
                }
            }
            Value::Vec(items) => self.vec_higher_order(items, name, args),
            Value::Native(iterator) if matches!(&*iterator.lock(), Native::Iterator(_)) => {
                self.iterator_higher_order(iterator, name, args)
            }
            Value::Enum { def, variant, data } if def.kind == EnumKind::Option => {
                self.option_higher_order(*variant, data, name, args)
            }
            Value::Enum { def, variant, data } if def.kind == EnumKind::Result => {
                self.result_higher_order(*variant, data, name, args)
            }
            Value::Native(n) if matches!(&*n.lock(), Native::Entry { .. }) => {
                let (map, key) = match &*n.lock() {
                    Native::Entry { map, key } => (map.clone(), key.clone()),
                    _ => unreachable!("checked by the guard"),
                };
                self.entry_higher_order(recv, &map, &key, name, args)
            }
            // `Value::as_str` hands a json string back pre unwrapped, so its Option closure
            // methods go here as Some
            Value::Str(s) => {
                let data: super::value::List =
                    Arc::new(parking_lot::Mutex::new(vec![Value::Str(s.clone())]));
                self.option_higher_order(SOME, &data, name, args)
            }
            _ => Ok(None),
        }
    }

    /// Non closure forms fall through to `entry_method`.
    pub(super) fn entry_higher_order(
        self: &Arc<Self>,
        entry: &Value,
        map: &Map,
        key: &MapKey,
        name: BuiltinId,
        args: &[Value],
    ) -> Result<Option<Value>> {
        match name {
            BuiltinId::OrInsertWith | BuiltinId::OrInsertWithKey => {
                let present = map.lock().contains_key(key);
                if !present {
                    let clo = as_closure(args.first())?;
                    let call_args = if name == BuiltinId::OrInsertWithKey {
                        vec![key.to_value()]
                    } else {
                        vec![]
                    };
                    let v = self.call_closure_data(&clo, &call_args)?;
                    map.lock().insert(key.clone(), v);
                }
                // `&mut V`, so writes must reach the map
                Ok(Some(Value::Ref(Arc::new(ValueRef::map_entry(
                    map.clone(),
                    key.clone(),
                )))))
            }
            BuiltinId::AndModify => {
                let current = map.lock().get(key).cloned();
                if let Some(current) = current {
                    let clo = as_closure(args.first())?;
                    let updated = self.call_closure_data(&clo, &[current])?;
                    // a unit return means it mutated in place
                    if !matches!(updated, Value::Unit) {
                        map.lock().insert(key.clone(), updated);
                    }
                }
                // the Entry, so `or_insert` still chains
                Ok(Some(entry.clone()))
            }
            _ => Ok(None),
        }
    }

    pub(super) fn vec_higher_order(
        self: &Arc<Self>,
        items: &List,
        name: BuiltinId,
        args: &[Value],
    ) -> Result<Option<Value>> {
        if let Some(v) = self.vec_transform_ho(items, name, args)? {
            return Ok(Some(v));
        }
        if let Some(v) = self.vec_reduce_ho(items, name, args)? {
            return Ok(Some(v));
        }
        self.vec_order_ho(items, name, args)
    }

    fn vec_transform_ho(
        self: &Arc<Self>,
        items: &List,
        name: BuiltinId,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let clo = |i: usize| as_closure(args.get(i));
        let list = items.lock().clone();
        let out = match name {
            BuiltinId::Map => {
                let f = clo(0)?;
                let mut r = Vec::with_capacity(list.len());
                for x in list {
                    r.push(self.call_closure_data(&f, &[x])?);
                }
                Value::vec(r)
            }
            BuiltinId::Filter => {
                let f = clo(0)?;
                let mut r = Vec::new();
                for x in list {
                    if self.call_closure_data(&f, from_ref(&x))?.is_truthy() {
                        r.push(x);
                    }
                }
                Value::vec(r)
            }
            BuiltinId::FilterMap => {
                let f = clo(0)?;
                let mut r = Vec::new();
                for x in list {
                    if let Some(inner) = option_inner(&self.call_closure_data(&f, &[x])?) {
                        r.push(inner);
                    }
                }
                Value::vec(r)
            }
            BuiltinId::FlatMap => {
                let f = clo(0)?;
                let mut r = Vec::new();
                for x in list {
                    r.extend(self.drain_items(self.call_closure_data(&f, &[x])?)?);
                }
                Value::vec(r)
            }
            BuiltinId::ForEach => {
                let f = clo(0)?;
                for x in list {
                    self.call_closure_data(&f, &[x])?;
                }
                Value::Unit
            }
            BuiltinId::TakeWhile => {
                let f = clo(0)?;
                let mut r = Vec::new();
                for x in list {
                    if self.call_closure_data(&f, from_ref(&x))?.is_truthy() {
                        r.push(x);
                    } else {
                        break;
                    }
                }
                Value::vec(r)
            }
            BuiltinId::SkipWhile => {
                let f = clo(0)?;
                let mut r = Vec::new();
                let mut skipping = true;
                for x in list {
                    if skipping && self.call_closure_data(&f, from_ref(&x))?.is_truthy() {
                        continue;
                    }
                    skipping = false;
                    r.push(x);
                }
                Value::vec(r)
            }
            BuiltinId::Partition => {
                let f = clo(0)?;
                let (mut yes, mut no) = (Vec::new(), Vec::new());
                for x in list {
                    if self.call_closure_data(&f, from_ref(&x))?.is_truthy() {
                        yes.push(x);
                    } else {
                        no.push(x);
                    }
                }
                Value::tuple(vec![Value::vec(yes), Value::vec(no)])
            }
            _ => return Ok(None),
        };
        Ok(Some(out))
    }

    fn vec_reduce_ho(
        self: &Arc<Self>,
        items: &List,
        name: BuiltinId,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let clo = |i: usize| as_closure(args.get(i));
        let list = items.lock().clone();
        let out = match name {
            BuiltinId::Find => {
                let f = clo(0)?;
                let mut found = Value::none();
                for x in list {
                    if self.call_closure_data(&f, from_ref(&x))?.is_truthy() {
                        found = Value::some(x);
                        break;
                    }
                }
                found
            }
            BuiltinId::FindMap => {
                let f = clo(0)?;
                let mut found = Value::none();
                for x in list {
                    if let Some(inner) = option_inner(&self.call_closure_data(&f, &[x])?) {
                        found = Value::some(inner);
                        break;
                    }
                }
                found
            }
            BuiltinId::Position => {
                let f = clo(0)?;
                let mut found = Value::none();
                for (i, x) in list.into_iter().enumerate() {
                    if self.call_closure_data(&f, &[x])?.is_truthy() {
                        found = Value::some(Value::Int(usize_i64(i)));
                        break;
                    }
                }
                found
            }
            BuiltinId::Any => {
                let f = clo(0)?;
                let mut any = false;
                for x in list {
                    if self.call_closure_data(&f, &[x])?.is_truthy() {
                        any = true;
                        break;
                    }
                }
                Value::Bool(any)
            }
            BuiltinId::All => {
                let f = clo(0)?;
                let mut all = true;
                for x in list {
                    if !self.call_closure_data(&f, &[x])?.is_truthy() {
                        all = false;
                        break;
                    }
                }
                Value::Bool(all)
            }
            BuiltinId::Fold => {
                let init = arg(args, 0)?;
                let f = clo(1)?;
                let mut acc = init;
                for x in list {
                    acc = self.call_closure_data(&f, &[acc, x])?;
                }
                acc
            }
            BuiltinId::Reduce => {
                let f = clo(0)?;
                let mut it = list.into_iter();
                match it.next() {
                    Some(first) => {
                        let mut acc = first;
                        for x in it {
                            acc = self.call_closure_data(&f, &[acc, x])?;
                        }
                        Value::some(acc)
                    }
                    None => Value::none(),
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(out))
    }

    fn vec_order_ho(
        self: &Arc<Self>,
        items: &List,
        name: BuiltinId,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let clo = |i: usize| as_closure(args.get(i));
        let list = items.lock().clone();
        let out = match name {
            BuiltinId::Retain => {
                let f = clo(0)?;
                let mut kept = Vec::new();
                for x in list {
                    if self.call_closure_data(&f, from_ref(&x))?.is_truthy() {
                        kept.push(x);
                    }
                }
                *items.lock() = kept;
                Value::Unit
            }
            BuiltinId::SortByKey | BuiltinId::SortByCachedKey => {
                let f = clo(0)?;
                let mut keyed = Vec::new();
                for x in list {
                    let k = self.call_closure_data(&f, from_ref(&x))?;
                    keyed.push((sort_key(&k), x));
                }
                keyed.sort_by(|a, b| a.0.cmp(&b.0));
                *items.lock() = keyed.into_iter().map(|(_, x)| x).collect();
                Value::Unit
            }
            BuiltinId::SortBy => {
                let f = clo(0)?;
                let mut sorted = list;
                let mut err = None;
                sorted.sort_by(|a, b| {
                    if err.is_some() {
                        return std::cmp::Ordering::Equal;
                    }
                    match self.call_closure_data(&f, &[a.clone(), b.clone()]) {
                        Ok(v) => ordering_from_value(&v).unwrap_or(std::cmp::Ordering::Equal),
                        Err(e) => {
                            err = Some(e);
                            std::cmp::Ordering::Equal
                        }
                    }
                });
                if let Some(e) = err {
                    return Err(e);
                }
                *items.lock() = sorted;
                Value::Unit
            }
            BuiltinId::MaxByKey | BuiltinId::MinByKey => {
                let f = clo(0)?;
                let want_max = name == BuiltinId::MaxByKey;
                let mut best: Option<(SortKey, Value)> = None;
                for x in list {
                    let k = sort_key(&self.call_closure_data(&f, from_ref(&x))?);
                    let take = match &best {
                        None => true,
                        Some((bk, _)) => {
                            if want_max {
                                k >= *bk
                            } else {
                                k < *bk
                            }
                        }
                    };
                    if take {
                        best = Some((k, x));
                    }
                }
                match best {
                    Some((_, x)) => Value::some(x),
                    None => Value::none(),
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(out))
    }

    pub(super) fn option_higher_order(
        self: &Arc<Self>,
        variant: u16,
        data: &super::value::List,
        name: BuiltinId,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let is_some = variant == SOME;
        let inner = || Value::payload(data);
        let clo = |i: usize| as_closure(args.get(i));
        let out = match name {
            BuiltinId::IsSomeAnd => {
                Value::Bool(is_some && self.call_closure_data(&clo(0)?, &[inner()?])?.is_truthy())
            }
            BuiltinId::Map => {
                if is_some {
                    Value::some(self.call_closure_data(&clo(0)?, &[inner()?])?)
                } else {
                    Value::none()
                }
            }
            BuiltinId::AndThen => {
                if is_some {
                    self.call_closure_data(&clo(0)?, &[inner()?])?
                } else {
                    Value::none()
                }
            }
            BuiltinId::Filter => {
                if is_some && self.call_closure_data(&clo(0)?, &[inner()?])?.is_truthy() {
                    Value::some(inner()?)
                } else {
                    Value::none()
                }
            }
            BuiltinId::MapOr => {
                let default = arg(args, 0)?;
                if is_some {
                    self.call_closure_data(&clo(1)?, &[inner()?])?
                } else {
                    default
                }
            }
            BuiltinId::MapOrElse => {
                if is_some {
                    self.call_closure_data(&clo(1)?, &[inner()?])?
                } else {
                    self.call_closure_data(&clo(0)?, &[])?
                }
            }
            BuiltinId::UnwrapOrElse => {
                if is_some {
                    inner()?
                } else {
                    self.call_closure_data(&clo(0)?, &[])?
                }
            }
            BuiltinId::OkOrElse | BuiltinId::WithContext => {
                if is_some {
                    Value::ok(inner()?)
                } else {
                    Value::err(self.call_closure_data(&clo(0)?, &[])?)
                }
            }
            BuiltinId::OrElse => {
                if is_some {
                    Value::some(inner()?)
                } else {
                    self.call_closure_data(&clo(0)?, &[])?
                }
            }
            BuiltinId::Or => {
                if is_some {
                    Value::some(inner()?)
                } else {
                    args.first().cloned().unwrap_or_else(Value::none)
                }
            }
            BuiltinId::And => {
                if is_some {
                    args.first().cloned().unwrap_or_else(Value::none)
                } else {
                    Value::none()
                }
            }
            BuiltinId::Xor => {
                let other = args.first().cloned().unwrap_or_else(Value::none);
                let other_some = other.is_enum_kind(EnumKind::Option)
                    && matches!(&other, Value::Enum { variant, .. } if *variant == SOME);
                match (is_some, other_some) {
                    (true, false) => Value::some(inner()?),
                    (false, true) => other,
                    _ => Value::none(),
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(out))
    }

    pub(super) fn result_higher_order(
        self: &Arc<Self>,
        variant: u16,
        data: &super::value::List,
        name: BuiltinId,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let is_ok = variant == OK;
        let inner = || Value::payload(data);
        let clo = |i: usize| as_closure(args.get(i));
        let out = match name {
            BuiltinId::IsOkAnd => {
                Value::Bool(is_ok && self.call_closure_data(&clo(0)?, &[inner()?])?.is_truthy())
            }
            BuiltinId::IsErrAnd => {
                Value::Bool(!is_ok && self.call_closure_data(&clo(0)?, &[inner()?])?.is_truthy())
            }
            BuiltinId::Map => {
                if is_ok {
                    Value::ok(self.call_closure_data(&clo(0)?, &[inner()?])?)
                } else {
                    Value::err(inner()?)
                }
            }
            BuiltinId::MapErr => {
                if is_ok {
                    Value::ok(inner()?)
                } else {
                    Value::err(self.call_closure_data(&clo(0)?, &[inner()?])?)
                }
            }
            BuiltinId::AndThen => {
                if is_ok {
                    self.call_closure_data(&clo(0)?, &[inner()?])?
                } else {
                    Value::err(inner()?)
                }
            }
            BuiltinId::MapOr => {
                let default = arg(args, 0)?;
                if is_ok {
                    self.call_closure_data(&clo(1)?, &[inner()?])?
                } else {
                    default
                }
            }
            // the fallback gets the error, unlike the Option form
            BuiltinId::MapOrElse => {
                if is_ok {
                    self.call_closure_data(&clo(1)?, &[inner()?])?
                } else {
                    self.call_closure_data(&clo(0)?, &[inner()?])?
                }
            }
            BuiltinId::UnwrapOrElse => {
                if is_ok {
                    inner()?
                } else {
                    self.call_closure_data(&clo(0)?, &[inner()?])?
                }
            }
            BuiltinId::WithContext => {
                if is_ok {
                    Value::ok(inner()?)
                } else {
                    let ctx = self.call_closure_data(&clo(0)?, &[])?.display();
                    Value::err(Value::str(format!(
                        "{ctx}\nCaused by: {}",
                        inner()?.display()
                    )))
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(out))
    }
}
