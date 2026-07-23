//! Builtin methods on the plain value types: String, Vec, HashMap,
//! numbers, Option, and Result. Split from `builtins.rs`.

use std::cell::RefCell;
use std::mem::take;
use std::rc::Rc;

use anyhow::{Result, anyhow, bail};

use super::bytecode::{BuiltinId, MethodName};

use super::value::{Map, RStr, StructData, Value};

use super::builtins::*;
use super::ops::compare_values;
use super::shared::{self, Args, CharOut, Num, NumOut, ParseNum, StrOut};

/// `map.entry(k).or_insert_with(Vec::new).push(x)` accumulates in place.
pub(super) fn entry_method(s: &StructData, name: &str, args: &[Value]) -> Result<Value> {
    let key = s
        .get("key")
        .and_then(|k| k.as_key())
        .ok_or_else(|| anyhow!("invalid entry key"))?;
    let Some(Value::Map(m)) = s.get("map") else {
        bail!("entry lost its map");
    };
    Ok(match name {
        "or_insert" => {
            let default = args.first().cloned().unwrap_or(Value::Unit);
            let mut map = m.borrow_mut();
            map.entry(key).or_insert(default).clone()
        }
        "or_default" => {
            let mut map = m.borrow_mut();
            map.entry(key)
                .or_insert_with(|| Value::vec(Vec::new()))
                .clone()
        }
        "key" => key.to_value(),
        _ => bail!("unknown method `{name}` on Entry"),
    })
}

/// The serde_json `is_*` family. A json value is a plain interpreter value
/// here, so each one is a type test. These apply to every receiver, so they are
/// answered before the per type dispatch, which returns early for the hot
/// receivers and would otherwise never reach them.
pub(super) fn json_type_test(recv: &Value, name: &str) -> Option<Value> {
    let is = |b: bool| Some(Value::Bool(b));
    match name {
        "is_object" => is(matches!(recv, Value::Map(_))),
        "is_array" => is(matches!(recv, Value::Vec(_))),
        "is_string" => is(matches!(recv, Value::Str(_))),
        "is_boolean" => is(matches!(recv, Value::Bool(_))),
        "is_number" => is(matches!(recv, Value::Int(_) | Value::Float(_))),
        "is_i64" | "is_u64" => is(matches!(recv, Value::Int(_))),
        "is_f64" => is(matches!(recv, Value::Float(_))),
        // The parser maps a json null to None, so that is what is_null has to
        // answer for. Unit counts too, it is the interpreter's own empty value.
        "is_null" => is(recv.is_none_value() || matches!(recv, Value::Unit)),
        _ => None,
    }
}

pub(super) fn generic_method(recv: &Value, name: &str, args: &[Value]) -> Result<Value> {
    match (recv, name) {
        (_, "clone") => Ok(recv.clone()),
        // Values are structurally typed here, so a conversion that only changes
        // the static type is a no-op. `vec.into()` for a `Cow<[u8]>` field is
        // the same vec. A receiver with a real conversion, an OsString into a
        // PathBuf for example, handles `into` in its own bridge before this.
        (_, "into") => Ok(recv.clone()),
        (_, "to_string") => Ok(Value::str(recv.display())),
        (Value::Char(ch), name) if let Some(out) = shared::char_method(*ch, name) => {
            Ok(match out {
                CharOut::Bool(v) => Value::Bool(v),
                CharOut::Char(c) => Value::Char(c),
                CharOut::Str(s) => Value::str(s),
            })
        }
        (Value::Bool(b), "as_bool") => Ok(Value::some(Value::Bool(*b))),
        // `then_some(v)` yields that value, not a placeholder.
        (Value::Bool(b), "then_some") => Ok(if *b {
            Value::some(args.first().cloned().unwrap_or(Value::Unit))
        } else {
            Value::none()
        }),
        (Value::Vec(v), "as_array") => Ok(Value::some(Value::vec(v.borrow().clone()))),
        // Serde accessors on a value that is not the matching type, for example
        // as_str on Null, are None rather than an error.
        (_, "as_str" | "as_i64" | "as_u64" | "as_f64" | "as_bool" | "as_array" | "as_object") => {
            Ok(Value::none())
        }
        // An enum names itself, so an unknown method on an Option says Option
        // and not the bare word enum. A struct names itself the same way.
        (Value::Enum { enum_name, .. }, _) => {
            bail!("unknown method `{name}` on {enum_name}")
        }
        (Value::Struct(s), _) => {
            bail!("unknown method `{name}` on struct `{}`", s.name())
        }
        _ => bail!("unknown method `{name}` on {}", recv.type_name()),
    }
}

pub(super) fn str_method(s: &Rc<RStr>, method: &MethodName, args: &[Value]) -> Result<Value> {
    use BuiltinId as B;
    let arg_str = |i: usize| -> String { args.get(i).map(|v| v.display()).unwrap_or_default() };
    Ok(match method.id {
        B::Len => Value::Int(s.len() as i64),
        B::IsEmpty => Value::Bool(s.is_empty()),
        B::Clone | B::ToString => Value::Str(s.clone()),
        B::Trim => Value::str(s.trim().to_string()),
        // Handled by the vm on the register slot, see Op::Method. Reaching
        // here means the receiver is not addressable, so the edit would be
        // silently lost.
        B::Push | B::PushStr => bail!("cannot mutate a string through this receiver"),
        B::Contains => Value::Bool(s.contains(&arg_str(0))),
        B::StartsWith => Value::Bool(s.starts_with(&arg_str(0))),
        B::EndsWith => Value::Bool(s.ends_with(&arg_str(0))),
        B::Chars => super::iterator::chars(s.clone()),
        B::Lines => super::iterator::lines(s.clone()),
        B::Split => split_value(s, args.first()),
        B::SplitWhitespace => super::iterator::split_whitespace(s.clone()),
        B::Count => Value::Int(s.chars().count() as i64),
        B::Parse => {
            let t = s.trim();
            if let Ok(i) = t.parse::<i64>() {
                Value::ok(Value::Int(i))
            } else if let Ok(f) = t.parse::<f64>() {
                Value::ok(Value::Float(f))
            } else if let Ok(b) = t.parse::<bool>() {
                Value::ok(Value::Bool(b))
            } else {
                Value::err(Value::str(format!("cannot parse `{t}`")))
            }
        }
        _ => return str_method_slow(s, &method.text, args),
    })
}

pub(super) fn str_method_slow(s: &Rc<RStr>, name: &str, args: &[Value]) -> Result<Value> {
    if let Some(out) = shared::str_core(s.as_str(), name, &VArgs(args))? {
        return Ok(str_out(s, out));
    }
    if let Some(text) = shared::color_core(s.as_str(), name) {
        return Ok(Value::str(text));
    }
    match name {
        // The lazy iterator form of the byte walk, engine specific by design.
        "bytes" => Ok(super::iterator::bytes(s.clone())),
        _ => generic_method(&Value::Str(s.clone()), name, args),
    }
}

/// Turn a neutral string core answer into a fast engine value. `Keep` clones
/// the `Rc`, so handing the receiver back stays a refcount bump.
fn str_out(s: &Rc<RStr>, out: StrOut) -> Value {
    match out {
        StrOut::Bool(b) => Value::Bool(b),
        StrOut::Int(i) => Value::Int(i),
        StrOut::Owned(o) => Value::str(o),
        StrOut::Keep => Value::Str(s.clone()),
        StrOut::OkKeep => Value::ok(Value::Str(s.clone())),
        StrOut::Strs(v) => Value::vec(v.into_iter().map(Value::str).collect()),
        StrOut::CharIdx(v) => Value::vec(
            v.into_iter()
                .map(|(i, c)| Value::tuple(vec![Value::Int(i), Value::Char(c)]))
                .collect(),
        ),
        StrOut::Ints(v) => Value::vec(v.into_iter().map(Value::Int).collect()),
        StrOut::OptOwned(o) => match o {
            Some(x) => Value::some(Value::str(x)),
            None => Value::none(),
        },
        StrOut::OptInt(o) => match o {
            Some(i) => Value::some(Value::Int(i)),
            None => Value::none(),
        },
        StrOut::OptPair(o) => match o {
            Some((x, y)) => Value::some(Value::tuple(vec![Value::str(x), Value::str(y)])),
            None => Value::none(),
        },
        StrOut::Ordering(o) => make_ordering(o),
        StrOut::Parse(p) => match p {
            ParseNum::Int(i) => Value::ok(Value::Int(i)),
            ParseNum::Float(f) => Value::ok(Value::Float(f)),
            ParseNum::Bool(b) => Value::ok(Value::Bool(b)),
            ParseNum::Fail(m) => Value::err(Value::str(m)),
        },
    }
}

/// The fast engine's argument view for the shared cores.
pub(super) struct VArgs<'a>(pub(super) &'a [Value]);

impl Args for VArgs<'_> {
    fn text(&self, i: usize) -> String {
        self.0.get(i).map(|v| v.display()).unwrap_or_default()
    }

    fn int(&self, i: usize) -> Option<i64> {
        match self.0.get(i) {
            Some(Value::Int(n)) => Some(*n),
            _ => None,
        }
    }

    fn float(&self, i: usize) -> Option<f64> {
        match self.0.get(i) {
            Some(Value::Float(f)) => Some(*f),
            Some(Value::Int(n)) => Some(*n as f64),
            _ => None,
        }
    }

    fn pattern_chars(&self, i: usize) -> Option<Vec<char>> {
        let Some(Value::Vec(items)) = self.0.get(i) else {
            return None;
        };
        Some(
            items
                .borrow()
                .iter()
                .filter_map(|v| match v {
                    Value::Char(c) => Some(*c),
                    Value::Str(text) => text.chars().next(),
                    _ => None,
                })
                .collect(),
        )
    }
}

pub(super) fn vec_method(
    v: &Rc<RefCell<Vec<Value>>>,
    method: &MethodName,
    args: &mut [Value],
) -> Result<Value> {
    use BuiltinId as B;
    Ok(match method.id {
        B::Len | B::Count => Value::Int(v.borrow().len() as i64),
        B::IsEmpty => Value::Bool(v.borrow().is_empty()),
        B::Clone => Value::vec(v.borrow().clone()),
        B::Iter => super::iterator::value_iter(v.clone()),
        B::Push => {
            v.borrow_mut()
                .push(args.first_mut().map(take).unwrap_or(Value::Unit));
            Value::Unit
        }
        B::Pop => match v.borrow_mut().pop() {
            Some(x) => Value::some(x),
            None => Value::none(),
        },
        B::Insert => {
            let i = int_arg(args, 0)? as usize;
            v.borrow_mut()
                .insert(i, args.get(1).cloned().unwrap_or(Value::Unit));
            Value::Unit
        }
        B::Remove => {
            let i = int_arg(args, 0)? as usize;
            v.borrow_mut().remove(i)
        }
        B::Get => {
            let i = int_arg(args, 0)? as usize;
            match v.borrow().get(i) {
                Some(x) => Value::some(x.clone()),
                None => Value::none(),
            }
        }
        B::First => v
            .borrow()
            .first()
            .cloned()
            .map(Value::some)
            .unwrap_or_else(Value::none),
        B::Last => v
            .borrow()
            .last()
            .cloned()
            .map(Value::some)
            .unwrap_or_else(Value::none),
        B::Contains => {
            let needle = args.first().cloned().unwrap_or(Value::Unit);
            Value::Bool(v.borrow().iter().any(|x| x.eq_value(&needle)))
        }
        B::Sort => {
            let mut items = v.borrow_mut();
            items.sort_by_key(sort_key);
            Value::Unit
        }
        B::Join => {
            let sep = args.first().map(|v| v.display()).unwrap_or_default();
            let joined = v
                .borrow()
                .iter()
                .map(|x| x.display())
                .collect::<Vec<_>>()
                .join(&sep);
            Value::str(joined)
        }
        // A vec of vecs flattens like the real slice `concat`; anything else
        // concatenates the display forms, which covers `Vec<String>`. The
        // empty case cannot know its element type, so it is a string.
        B::Concat => {
            let items = v.borrow();
            match items.first() {
                Some(Value::Vec(_)) => {
                    let mut out = Vec::new();
                    for x in items.iter() {
                        if let Value::Vec(inner) = x {
                            out.extend(inner.borrow().iter().cloned());
                        }
                    }
                    Value::vec(out)
                }
                _ => Value::str(items.iter().map(|x| x.display()).collect::<String>()),
            }
        }
        B::Sum => {
            let mut acc_i = 0i64;
            let mut acc_f = 0f64;
            let mut is_float = false;
            for x in v.borrow().iter() {
                match x {
                    Value::Int(i) => acc_i += i,
                    Value::Float(f) => {
                        is_float = true;
                        acc_f += f;
                    }
                    _ => bail!("sum needs numbers"),
                }
            }
            if is_float {
                Value::Float(acc_f + acc_i as f64)
            } else {
                Value::Int(acc_i)
            }
        }
        B::Rev => {
            let mut items = v.borrow().clone();
            items.reverse();
            Value::vec(items)
        }
        B::Enumerate => Value::vec(
            v.borrow()
                .iter()
                .enumerate()
                .map(|(i, x)| Value::tuple(vec![Value::Int(i as i64), x.clone()]))
                .collect(),
        ),
        B::Take => {
            let n = int_arg(args, 0)? as usize;
            Value::vec(v.borrow().iter().take(n).cloned().collect())
        }
        B::Skip => {
            let n = int_arg(args, 0)? as usize;
            Value::vec(v.borrow().iter().skip(n).cloned().collect())
        }
        _ => match method.text.as_str() {
            "to_vec" | "collect" | "cloned" | "copied" => Value::vec(v.borrow().clone()),
            "nth" => match v.borrow().get(int_arg(args, 0)? as usize) {
                Some(item) => Value::some(item.clone()),
                None => Value::none(),
            },
            "collect_string" => {
                Value::str(v.borrow().iter().map(Value::display).collect::<String>())
            }
            "reverse" => {
                v.borrow_mut().reverse();
                Value::Unit
            }
            "dedup" => {
                let mut items = v.borrow_mut();
                items.dedup_by(|a, b| a.eq_value(b));
                Value::Unit
            }
            "clear" => {
                v.borrow_mut().clear();
                Value::Unit
            }
            // Compiled from `v[a..b].copy_from_slice(src)` with the bounds as
            // leading args, so the write reaches the base vec instead of a
            // copied slice temporary. An open end arrives as the max sentinel.
            "copy_from_slice" => {
                let start = int_arg(args, 0)? as usize;
                let end_raw = int_arg(args, 1)?;
                let src: Vec<Value> = match args.get(2) {
                    Some(Value::Vec(other)) => other.borrow().clone(),
                    _ => bail!("copy_from_slice takes a slice argument"),
                };
                let mut items = v.borrow_mut();
                let end = if end_raw == i64::MAX {
                    items.len()
                } else {
                    end_raw as usize
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
                Value::Unit
            }
            "truncate" => {
                let n = int_arg(args, 0)? as usize;
                v.borrow_mut().truncate(n);
                Value::Unit
            }
            "extend" | "append" | "extend_from_slice" => {
                if let Some(Value::Vec(other)) = args.first() {
                    v.borrow_mut().extend(other.borrow().iter().cloned());
                }
                Value::Unit
            }
            // Flattens one level: nested vectors spill their items, and Ok/Some
            // yield their inner value while Err/None drop out.
            "flatten" => {
                let items = v.borrow();
                let mut out: Vec<Value> = Vec::new();
                for item in items.iter() {
                    match item {
                        Value::Vec(inner) => out.extend(inner.borrow().iter().cloned()),
                        Value::Enum { variant, data, .. }
                            if matches!(&**variant, "Some" | "Ok") =>
                        {
                            if let Some(inner) = data.first() {
                                out.push(inner.clone());
                            }
                        }
                        Value::Enum { variant, .. } if matches!(&**variant, "None" | "Err") => {}
                        other => out.push(other.clone()),
                    }
                }
                Value::vec(out)
            }
            // Iterators are eager vectors here, so `next` is the first item.
            // The check gate keeps it off real vectors, where it won't compile.
            "next" => v
                .borrow()
                .first()
                .cloned()
                .map(Value::some)
                .unwrap_or_else(Value::none),
            "max" | "min" => {
                let items = v.borrow();
                let mut best: Option<&Value> = None;
                for item in items.iter() {
                    let better = match best {
                        Some(b) => {
                            let ord = compare_values(item, b)?;
                            if method.text == "max" {
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
                best.cloned().map(Value::some).unwrap_or_else(Value::none)
            }
            // A JSON array parsed by the interpreter is a plain Vec, so the
            // serde_json accessors resolve against it here.
            _ => match method.text.as_str() {
                "as_array" => Value::some(Value::vec(v.borrow().clone())),
                "as_object" => Value::none(),
                // Names that apply to any receiver, `clone` and `into` and the
                // rest, live in one place instead of being repeated per type.
                other => return generic_method(&Value::Vec(v.clone()), other, args),
            },
        },
    })
}

pub(super) fn map_method(
    m: &Rc<RefCell<Map>>,
    method: &MethodName,
    args: &mut [Value],
) -> Result<Value> {
    use BuiltinId as B;
    // Read-only lookups borrow the key instead of cloning it.
    let lookup = |i: usize, f: &dyn Fn(Option<&Value>) -> Value| -> Result<Value> {
        let arg = args.get(i).ok_or_else(|| anyhow!("invalid map key"))?;
        let k = arg.key_ref().ok_or_else(|| anyhow!("invalid map key"))?;
        Ok(f(m.borrow().get(&k)))
    };
    Ok(match method.id {
        B::Len | B::Count => Value::Int(m.borrow().len() as i64),
        B::IsEmpty => Value::Bool(m.borrow().is_empty()),
        B::Clone => Value::Map(Rc::new(RefCell::new(m.borrow().clone()))),
        B::Insert => {
            let k = take(&mut args[0])
                .into_key()
                .ok_or_else(|| anyhow!("invalid map key"))?;
            let val = args.get_mut(1).map(take).unwrap_or(Value::Unit);
            let old = m.borrow_mut().insert(k, val);
            match old {
                Some(v) => Value::some(v),
                None => Value::none(),
            }
        }
        B::Get => lookup(0, &|v| match v {
            Some(v) => Value::some(v.clone()),
            None => Value::none(),
        })?,
        B::ContainsKey => lookup(0, &|v| Value::Bool(v.is_some()))?,
        B::Remove => {
            let arg = args.first().ok_or_else(|| anyhow!("invalid map key"))?;
            let k = arg.key_ref().ok_or_else(|| anyhow!("invalid map key"))?;
            let removed = m.borrow_mut().shift_remove(&k);
            match removed {
                Some(v) => Value::some(v),
                None => Value::none(),
            }
        }
        B::Keys => Value::vec(m.borrow().keys().map(|k| k.to_value()).collect()),
        B::Values => Value::vec(m.borrow().values().cloned().collect()),
        B::Entry => Value::struct_of(
            "Entry",
            [
                ("map".into(), Value::Map(m.clone())),
                ("key".into(), args.first().cloned().unwrap_or(Value::Unit)),
            ],
        ),
        B::Iter => map_pairs(m),
        _ => match method.text.as_str() {
            "values_mut" => Value::vec(m.borrow().values().cloned().collect()),
            "drain" => map_pairs(m),
            // A JSON object parsed by the interpreter is a Map, so the
            // serde_json accessors resolve against it here.
            "as_object" => Value::some(Value::Map(m.clone())),
            "as_array" => Value::none(),
            name => return generic_method(&Value::Map(m.clone()), name, &*args),
        },
    })
}

pub(super) fn map_pairs(m: &Rc<RefCell<Map>>) -> Value {
    Value::vec(
        m.borrow()
            .iter()
            .map(|(k, v)| Value::tuple(vec![k.to_value(), v.clone()]))
            .collect(),
    )
}

pub(super) fn num_method(recv: &Value, name: &str, args: &[Value]) -> Result<Value> {
    match name {
        "to_string" => return Ok(Value::str(recv.display())),
        "clone" => return Ok(recv.clone()),
        _ => {}
    }
    let n = match recv {
        Value::Int(i) => Num::Int(*i),
        Value::Float(f) => Num::Float(*f),
        _ => bail!("unknown numeric method `{name}`"),
    };
    match shared::num_core(n, name, &VArgs(args))? {
        Some(out) => Ok(num_out(out)),
        None => bail!("unknown numeric method `{name}`"),
    }
}

/// Turn a neutral numeric core answer into a fast engine value.
fn num_out(out: NumOut) -> Value {
    match out {
        NumOut::Int(i) => Value::Int(i),
        NumOut::Float(f) => Value::Float(f),
        NumOut::Bool(b) => Value::Bool(b),
        NumOut::SomeInt(i) => Value::some(Value::Int(i)),
        NumOut::SomeFloat(f) => Value::some(Value::Float(f)),
        NumOut::Nothing => Value::none(),
        NumOut::Ordering(o) => make_ordering(o),
        NumOut::SomeOrdering(o) => Value::some(make_ordering(o)),
    }
}

pub(super) fn opt_method(recv: &Value, method: &MethodName, args: &[Value]) -> Result<Value> {
    use BuiltinId as B;
    // The hot accessors dispatch on the id before the variant is even looked
    // at, and the payload is cloned only on the paths that hand it out.
    if let B::Clone | B::Copied = method.id {
        return Ok(recv.clone());
    }
    let (is_some, inner) = match recv {
        Value::Enum { variant, data, .. } => (&**variant == "Some", data.first().cloned()),
        _ => unreachable!(),
    };
    match method.id {
        B::Unwrap => {
            return inner.ok_or_else(|| anyhow!("called `Option::unwrap()` on a `None` value"));
        }
        B::UnwrapOr => {
            return Ok(inner.unwrap_or_else(|| args.first().cloned().unwrap_or(Value::Unit)));
        }
        _ => {}
    }
    let name = method.text.as_str();
    Ok(match name {
        "is_some" => Value::Bool(is_some),
        "is_none" => Value::Bool(!is_some),
        "expect" => inner
            .ok_or_else(|| anyhow!("{}", args.first().map(|v| v.display()).unwrap_or_default()))?,
        // There is no runtime type here, so the Ok type's Default cannot be
        // built. Scripts use this almost only on string results such as
        // read_to_string and env::var, so an empty string is the practical
        // default. For another type use unwrap_or with an explicit value.
        "unwrap_or_default" => inner.unwrap_or_else(|| Value::str(String::new())),
        "as_ref" | "as_deref" | "take" | "as_mut" => recv.clone(),
        // A json null parses to None here, so a serde lookup into a value that
        // turned out to be null is None rather than an unknown method error.
        "get" => Value::none(),
        "ok_or" => match inner {
            Some(v) => Value::ok(v),
            None => Value::err(args.first().cloned().unwrap_or(Value::Unit)),
        },
        "context" => match inner {
            Some(v) => Value::ok(v),
            None => Value::err(args.first().cloned().unwrap_or(Value::Unit)),
        },
        _ => return generic_method(recv, method.text.as_str(), args),
    })
}

pub(super) fn res_method(recv: &Value, method: &MethodName, args: &[Value]) -> Result<Value> {
    let (is_ok, inner) = match recv {
        Value::Enum { variant, data, .. } => (&**variant == "Ok", data.first().cloned()),
        _ => unreachable!(),
    };
    let name = method.text.as_str();
    Ok(match name {
        "is_ok" => Value::Bool(is_ok),
        "is_err" => Value::Bool(!is_ok),
        "clone" => recv.clone(),
        // The interpreter holds no references, so a reference view is the value.
        "as_ref" | "as_mut" | "as_deref" | "as_deref_mut" => recv.clone(),
        "unwrap" => {
            if is_ok {
                inner.unwrap_or(Value::Unit)
            } else {
                bail!(
                    "called `Result::unwrap()` on an `Err` value: {}",
                    inner.map(|v| v.debug()).unwrap_or_default()
                );
            }
        }
        "unwrap_err" => {
            if is_ok {
                bail!(
                    "called `Result::unwrap_err()` on an `Ok` value: {}",
                    inner.map(|v| v.debug()).unwrap_or_default()
                );
            } else {
                inner.unwrap_or(Value::Unit)
            }
        }
        "expect" => {
            if is_ok {
                inner.unwrap_or(Value::Unit)
            } else {
                bail!("{}", args.first().map(|v| v.display()).unwrap_or_default());
            }
        }
        "unwrap_or" => {
            if is_ok {
                inner.unwrap_or(Value::Unit)
            } else {
                args.first().cloned().unwrap_or(Value::Unit)
            }
        }
        // Same string-default reasoning as Option::unwrap_or_default above.
        "unwrap_or_default" => {
            if is_ok {
                inner.unwrap_or_else(|| Value::str(String::new()))
            } else {
                Value::str(String::new())
            }
        }
        "ok" => {
            if is_ok {
                Value::some(inner.unwrap_or(Value::Unit))
            } else {
                Value::none()
            }
        }
        "err" => {
            if is_ok {
                Value::none()
            } else {
                Value::some(inner.unwrap_or(Value::Unit))
            }
        }
        "context" | "with_context" => {
            if is_ok {
                Value::ok(inner.unwrap_or(Value::Unit))
            } else {
                let ctx = args.first().map(|v| v.display()).unwrap_or_default();
                let cause = inner.map(|v| v.display()).unwrap_or_default();
                Value::err(Value::str(format!("{ctx}\nCaused by: {cause}")))
            }
        }
        _ => return generic_method(recv, method.text.as_str(), args),
    })
}

pub(super) fn int_arg(args: &[Value], i: usize) -> Result<i64> {
    match args.get(i) {
        Some(Value::Int(n)) => Ok(*n),
        _ => bail!("expected an integer argument"),
    }
}

/// Ordering key for `sort`, good enough for numbers and strings.
pub(super) fn sort_key(v: &Value) -> SortKey {
    match v {
        Value::Int(i) => SortKey::Int(*i),
        Value::Float(f) => SortKey::Float(*f),
        Value::Bool(b) => SortKey::Int(*b as i64),
        Value::Str(s) => SortKey::Str(s.to_string()),
        Value::Char(c) => SortKey::Str(c.to_string()),
        Value::Tuple(items) | Value::Vec(items) => {
            SortKey::List(items.borrow().iter().map(sort_key).collect())
        }
        other => SortKey::Str(other.display()),
    }
}

#[derive(PartialEq)]
pub(super) enum SortKey {
    Int(i64),
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
            (SortKey::Int(a), SortKey::Float(b)) => {
                (*a as f64).partial_cmp(b).unwrap_or(Ordering::Equal)
            }
            (SortKey::Float(a), SortKey::Int(b)) => {
                a.partial_cmp(&(*b as f64)).unwrap_or(Ordering::Equal)
            }
            (SortKey::Str(a), SortKey::Str(b)) => a.cmp(b),
            (SortKey::List(a), SortKey::List(b)) => a.cmp(b),
            (SortKey::Int(_) | SortKey::Float(_), _) => Ordering::Less,
            (_, SortKey::Int(_) | SortKey::Float(_)) => Ordering::Greater,
            (SortKey::Str(_), SortKey::List(_)) => Ordering::Less,
            (SortKey::List(_), SortKey::Str(_)) => Ordering::Greater,
        }
    }
}

/// `str::split` with either a string pattern or a set of chars. A char array
/// like `['-', '_']` splits on any of them, which a plain string pattern would
/// otherwise match only as the literal sequence.
pub(super) fn split_value(s: &Rc<RStr>, pattern: Option<&Value>) -> Value {
    if let Some(Value::Vec(items)) = pattern {
        let chars: Vec<char> = items
            .borrow()
            .iter()
            .filter_map(|v| match v {
                Value::Char(c) => Some(*c),
                Value::Str(text) => text.chars().next(),
                _ => None,
            })
            .collect();
        return Value::vec(
            s.split(|c: char| chars.contains(&c))
                .map(Value::str)
                .collect(),
        );
    }
    let sep = pattern.map(Value::display).unwrap_or_default();
    Value::vec(s.split(&sep).map(Value::str).collect())
}
