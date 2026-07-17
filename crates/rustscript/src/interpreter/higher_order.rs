//! The closure taking methods on Vec, HashMap entries, Option, and
//! Result: map, filter, fold and friends. Split from `builtins.rs`.

use std::cell::RefCell;
use std::rc::Rc;
use std::slice::from_ref;

use anyhow::{Result, anyhow, bail};

use super::Interp;
use super::value::{StructData, Value};

use super::builtins::*;
use super::methods::*;

impl Interp {
    /// Methods that take a closure, on Vec, Option, and Result. Returns None
    /// when the method is not one of these, so plain dispatch can handle it.
    pub(super) fn higher_order(
        &self,
        recv: &Value,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>> {
        match recv {
            Value::Vec(items) => self.vec_higher_order(items, name, args),
            Value::Native(iterator)
                if matches!(&*iterator.borrow(), super::native::Native::Iterator(_)) =>
            {
                self.iterator_higher_order(iterator, name, args)
            }
            Value::Enum {
                enum_name,
                variant,
                data,
            } if &**enum_name == "Option" => self.option_higher_order(variant, data, name, args),
            Value::Enum {
                enum_name,
                variant,
                data,
            } if &**enum_name == "Result" => self.result_higher_order(variant, data, name, args),
            Value::Struct(s) if &**s.name() == "Entry" => self.entry_higher_order(s, name, args),
            // A JSON string is a plain String, but Value::as_str hands it back as
            // an already unwrapped Some, so its Option closure methods route here
            // as Some. Unknown names fall through to Ok(None) and plain dispatch.
            Value::Str(s) => {
                let data: Rc<[Value]> = Rc::from([Value::Str(s.clone())]);
                self.option_higher_order("Some", &data, name, args)
            }
            _ => Ok(None),
        }
    }

    /// The closure forms of `HashMap::entry`: `or_insert_with`, `or_insert_with_key`,
    /// and `and_modify`. Non-closure forms fall through to `entry_method`.
    pub(super) fn entry_higher_order(
        &self,
        entry: &Rc<StructData>,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let key = entry
            .get("key")
            .and_then(|k| k.as_key())
            .ok_or_else(|| anyhow!("invalid entry key"))?;
        let Some(Value::Map(map)) = entry.get("map") else {
            bail!("entry lost its map");
        };
        match name {
            "or_insert_with" | "or_insert_with_key" => {
                let present = map.borrow().contains_key(&key);
                if !present {
                    let clo = as_closure(args.first())?;
                    let call_args = if name == "or_insert_with_key" {
                        vec![key.to_value()]
                    } else {
                        vec![]
                    };
                    let v = self.call_closure(&clo, &call_args)?;
                    map.borrow_mut().insert(key.clone(), v);
                }
                Ok(Some(map.borrow().get(&key).cloned().unwrap_or(Value::Unit)))
            }
            "and_modify" => {
                if map.borrow().contains_key(&key) {
                    let clo = as_closure(args.first())?;
                    let current = map.borrow().get(&key).cloned().unwrap_or(Value::Unit);
                    let updated = self.call_closure(&clo, &[current])?;
                    // A closure that returns unit means it mutated in place via
                    // a shared container; otherwise take its return value.
                    if !matches!(updated, Value::Unit) {
                        map.borrow_mut().insert(key.clone(), updated);
                    }
                }
                // Return the Entry so further chaining (or_insert) still works.
                Ok(Some(Value::Struct(entry.clone())))
            }
            _ => Ok(None),
        }
    }

    pub(super) fn vec_higher_order(
        &self,
        items: &Rc<RefCell<Vec<Value>>>,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let clo = |i: usize| as_closure(args.get(i));
        let list = items.borrow().clone();
        let out = match name {
            "map" => {
                let f = clo(0)?;
                let mut r = Vec::with_capacity(list.len());
                for x in list {
                    r.push(self.call_closure(&f, &[x])?);
                }
                Value::vec(r)
            }
            "filter" => {
                let f = clo(0)?;
                let mut r = Vec::new();
                for x in list {
                    if self.call_closure(&f, from_ref(&x))?.is_truthy() {
                        r.push(x);
                    }
                }
                Value::vec(r)
            }
            "filter_map" => {
                let f = clo(0)?;
                let mut r = Vec::new();
                for x in list {
                    if let Some(inner) = option_inner(&self.call_closure(&f, &[x])?) {
                        r.push(inner);
                    }
                }
                Value::vec(r)
            }
            "flat_map" => {
                let f = clo(0)?;
                let mut r = Vec::new();
                for x in list {
                    r.extend(self.iter_items(self.call_closure(&f, &[x])?)?);
                }
                Value::vec(r)
            }
            "for_each" => {
                let f = clo(0)?;
                for x in list {
                    self.call_closure(&f, &[x])?;
                }
                Value::Unit
            }
            "find" => {
                let f = clo(0)?;
                let mut found = Value::none();
                for x in list {
                    if self.call_closure(&f, from_ref(&x))?.is_truthy() {
                        found = Value::some(x);
                        break;
                    }
                }
                found
            }
            "position" => {
                let f = clo(0)?;
                let mut found = Value::none();
                for (i, x) in list.into_iter().enumerate() {
                    if self.call_closure(&f, &[x])?.is_truthy() {
                        found = Value::some(Value::Int(i as i64));
                        break;
                    }
                }
                found
            }
            "any" => {
                let f = clo(0)?;
                let mut any = false;
                for x in list {
                    if self.call_closure(&f, &[x])?.is_truthy() {
                        any = true;
                        break;
                    }
                }
                Value::Bool(any)
            }
            "all" => {
                let f = clo(0)?;
                let mut all = true;
                for x in list {
                    if !self.call_closure(&f, &[x])?.is_truthy() {
                        all = false;
                        break;
                    }
                }
                Value::Bool(all)
            }
            "fold" => {
                let init = args.first().cloned().unwrap_or(Value::Unit);
                let f = clo(1)?;
                let mut acc = init;
                for x in list {
                    acc = self.call_closure(&f, &[acc, x])?;
                }
                acc
            }
            "reduce" => {
                let f = clo(0)?;
                let mut it = list.into_iter();
                match it.next() {
                    Some(first) => {
                        let mut acc = first;
                        for x in it {
                            acc = self.call_closure(&f, &[acc, x])?;
                        }
                        Value::some(acc)
                    }
                    None => Value::none(),
                }
            }
            "retain" => {
                let f = clo(0)?;
                let mut kept = Vec::new();
                for x in list {
                    if self.call_closure(&f, from_ref(&x))?.is_truthy() {
                        kept.push(x);
                    }
                }
                *items.borrow_mut() = kept;
                Value::Unit
            }
            "sort_by_key" | "sort_by_cached_key" => {
                let f = clo(0)?;
                let mut runner = self.closure_runner(f);
                let mut keyed = Vec::new();
                for x in list {
                    let k = runner.call_refs(&[&x])?;
                    keyed.push((sort_key(&k), x));
                }
                keyed.sort_by(|a, b| a.0.cmp(&b.0));
                *items.borrow_mut() = keyed.into_iter().map(|(_, x)| x).collect();
                Value::Unit
            }
            "sort_by" => {
                let f = clo(0)?;
                let mut runner = self.closure_runner(f);
                let mut sorted = list;
                let mut err = None;
                sorted.sort_by(|a, b| {
                    if err.is_some() {
                        return std::cmp::Ordering::Equal;
                    }
                    match runner.call_refs(&[a, b]) {
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
                *items.borrow_mut() = sorted;
                Value::Unit
            }
            "max_by_key" | "min_by_key" => {
                let f = clo(0)?;
                let want_max = name == "max_by_key";
                let mut best: Option<(SortKey, Value)> = None;
                for x in list {
                    let k = sort_key(&self.call_closure(&f, from_ref(&x))?);
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
            "take_while" => {
                let f = clo(0)?;
                let mut r = Vec::new();
                for x in list {
                    if self.call_closure(&f, from_ref(&x))?.is_truthy() {
                        r.push(x);
                    } else {
                        break;
                    }
                }
                Value::vec(r)
            }
            "skip_while" => {
                let f = clo(0)?;
                let mut r = Vec::new();
                let mut skipping = true;
                for x in list {
                    if skipping && self.call_closure(&f, from_ref(&x))?.is_truthy() {
                        continue;
                    }
                    skipping = false;
                    r.push(x);
                }
                Value::vec(r)
            }
            "partition" => {
                let f = clo(0)?;
                let (mut yes, mut no) = (Vec::new(), Vec::new());
                for x in list {
                    if self.call_closure(&f, from_ref(&x))?.is_truthy() {
                        yes.push(x);
                    } else {
                        no.push(x);
                    }
                }
                Value::Tuple(Rc::new(RefCell::new(vec![Value::vec(yes), Value::vec(no)])))
            }
            _ => return Ok(None),
        };
        Ok(Some(out))
    }

    pub(super) fn option_higher_order(
        &self,
        variant: &str,
        data: &Rc<[Value]>,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let is_some = variant == "Some";
        let inner = || data.first().cloned().unwrap_or(Value::Unit);
        let clo = |i: usize| as_closure(args.get(i));
        let out = match name {
            "is_some_and" => {
                Value::Bool(is_some && self.call_closure(&*clo(0)?, &[inner()])?.is_truthy())
            }
            "map" => {
                if is_some {
                    Value::some(self.call_closure(&*clo(0)?, &[inner()])?)
                } else {
                    Value::none()
                }
            }
            "and_then" => {
                if is_some {
                    self.call_closure(&*clo(0)?, &[inner()])?
                } else {
                    Value::none()
                }
            }
            "filter" => {
                if is_some && self.call_closure(&*clo(0)?, &[inner()])?.is_truthy() {
                    Value::some(inner())
                } else {
                    Value::none()
                }
            }
            "map_or" => {
                let default = args.first().cloned().unwrap_or(Value::Unit);
                if is_some {
                    self.call_closure(&*clo(1)?, &[inner()])?
                } else {
                    default
                }
            }
            "map_or_else" => {
                if is_some {
                    self.call_closure(&*clo(1)?, &[inner()])?
                } else {
                    self.call_closure(&*clo(0)?, &[])?
                }
            }
            "unwrap_or_else" => {
                if is_some {
                    inner()
                } else {
                    self.call_closure(&*clo(0)?, &[])?
                }
            }
            "ok_or_else" => {
                if is_some {
                    Value::ok(inner())
                } else {
                    Value::err(self.call_closure(&*clo(0)?, &[])?)
                }
            }
            "with_context" => {
                if is_some {
                    Value::ok(inner())
                } else {
                    Value::err(self.call_closure(&*clo(0)?, &[])?)
                }
            }
            "or_else" => {
                if is_some {
                    Value::some(inner())
                } else {
                    self.call_closure(&*clo(0)?, &[])?
                }
            }
            "or" => {
                if is_some {
                    Value::some(inner())
                } else {
                    args.first().cloned().unwrap_or_else(Value::none)
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(out))
    }

    pub(super) fn result_higher_order(
        &self,
        variant: &str,
        data: &Rc<[Value]>,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let is_ok = variant == "Ok";
        let inner = || data.first().cloned().unwrap_or(Value::Unit);
        let clo = |i: usize| as_closure(args.get(i));
        let out = match name {
            "is_ok_and" => {
                Value::Bool(is_ok && self.call_closure(&*clo(0)?, &[inner()])?.is_truthy())
            }
            "is_err_and" => {
                Value::Bool(!is_ok && self.call_closure(&*clo(0)?, &[inner()])?.is_truthy())
            }
            "map" => {
                if is_ok {
                    Value::ok(self.call_closure(&*clo(0)?, &[inner()])?)
                } else {
                    Value::err(inner())
                }
            }
            "map_err" => {
                if is_ok {
                    Value::ok(inner())
                } else {
                    Value::err(self.call_closure(&*clo(0)?, &[inner()])?)
                }
            }
            "and_then" => {
                if is_ok {
                    self.call_closure(&*clo(0)?, &[inner()])?
                } else {
                    Value::err(inner())
                }
            }
            "unwrap_or_else" => {
                if is_ok {
                    inner()
                } else {
                    self.call_closure(&*clo(0)?, &[inner()])?
                }
            }
            "with_context" => {
                if is_ok {
                    Value::ok(inner())
                } else {
                    let ctx = self.call_closure(&*clo(0)?, &[])?.display();
                    Value::err(Value::str(format!(
                        "{ctx}\nCaused by: {}",
                        inner().display()
                    )))
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(out))
    }
}
