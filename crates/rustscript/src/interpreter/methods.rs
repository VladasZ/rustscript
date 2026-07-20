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
use super::std_bridge::bytes_to_vec;

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
        "is_null" => is(matches!(recv, Value::Unit)),
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
        (Value::Char(ch), name) if let Some(out) = char_method(*ch, name) => Ok(match out {
            CharOut::Bool(v) => Value::Bool(v),
            CharOut::Char(c) => Value::Char(c),
            CharOut::Str(s) => Value::str(s),
        }),
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
        // and not the bare word enum.
        (Value::Enum { enum_name, .. }, _) => {
            bail!("unknown method `{name}` on {enum_name}")
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
    let arg_str = |i: usize| -> String { args.get(i).map(|v| v.display()).unwrap_or_default() };
    Ok(match name {
        "to_owned" | "trim_string" => Value::Str(s.clone()),
        "to_uppercase" | "to_ascii_uppercase" => Value::str(s.to_uppercase()),
        "to_lowercase" | "to_ascii_lowercase" => Value::str(s.to_lowercase()),
        "trim_start" => Value::str(s.trim_start().to_string()),
        "trim_end" => Value::str(s.trim_end().to_string()),
        "replace" => Value::str(s.replace(&arg_str(0), &arg_str(1))),
        "replacen" => Value::str(s.replacen(&arg_str(0), &arg_str(1), int_arg(args, 2)? as usize)),
        "repeat" => {
            let n = match args.first() {
                Some(Value::Int(n)) => *n as usize,
                _ => 0,
            };
            Value::str(s.repeat(n))
        }
        // String::as_str gives the string back. serde_json::Value::as_str
        // gives an Option, and a json string is a plain Str here, so unwrap
        // and expect on a string are identity to keep that pattern working.
        // A JSON string parsed by the interpreter is a plain String, so the
        // serde_json Value::as_str chains resolve by treating the string as an
        // already-unwrapped Some. as_str keeps the string, and the Option
        // consumers below hand it straight back.
        "as_str" | "as_string" | "unwrap" | "expect" => Value::Str(s.clone()),
        "unwrap_or" | "unwrap_or_else" | "unwrap_or_default" => Value::Str(s.clone()),
        // `Option::context` returns a Result, so the pre-unwrapped string has to
        // come back wrapped or a following `?` would have nothing to unwrap.
        "context" | "with_context" => Value::ok(Value::Str(s.clone())),
        "is_some" => Value::Bool(true),
        "is_none" => Value::Bool(false),
        "as_bytes" | "into_bytes" => bytes_to_vec(s.as_bytes()),
        "bytes" => super::iterator::bytes(s.clone()),
        // The utf-16 code units as an eager Vec of ints, mirroring how `bytes`
        // gives the utf-8 bytes. std hands back an iterator of u16, we collect.
        "encode_utf16" => Value::vec(s.encode_utf16().map(|u| Value::Int(i64::from(u))).collect()),
        "strip_prefix" => match s.strip_prefix(&arg_str(0)) {
            Some(rest) => Value::some(Value::str(rest.to_string())),
            None => Value::none(),
        },
        "strip_suffix" => match s.strip_suffix(&arg_str(0)) {
            Some(rest) => Value::some(Value::str(rest.to_string())),
            None => Value::none(),
        },
        // Byte offsets, same as the real std, and the slicing here is
        // byte-based too, so `&s[..s.find(x).unwrap()]` behaves right.
        "find" => match s.find(&arg_str(0)) {
            Some(i) => Value::some(Value::Int(i as i64)),
            None => Value::none(),
        },
        "rfind" => match s.rfind(&arg_str(0)) {
            Some(i) => Value::some(Value::Int(i as i64)),
            None => Value::none(),
        },
        "split_once" => match s.split_once(&arg_str(0)) {
            Some((a, b)) => Value::some(Value::tuple(vec![
                Value::str(a.to_string()),
                Value::str(b.to_string()),
            ])),
            None => Value::none(),
        },
        "rsplit_once" => match s.rsplit_once(&arg_str(0)) {
            Some((a, b)) => Value::some(Value::tuple(vec![
                Value::str(a.to_string()),
                Value::str(b.to_string()),
            ])),
            None => Value::none(),
        },
        "rsplit" => {
            let sep = arg_str(0);
            Value::vec(s.rsplit(&sep).map(Value::str).collect())
        }
        "splitn" => {
            let n = int_arg(args, 0)? as usize;
            Value::vec(s.splitn(n, &arg_str(1)).map(Value::str).collect())
        }
        "rsplitn" => {
            let n = int_arg(args, 0)? as usize;
            Value::vec(s.rsplitn(n, &arg_str(1)).map(Value::str).collect())
        }
        "trim_matches" | "trim_start_matches" | "trim_end_matches" => {
            let pat = arg_str(0);
            let out = match name {
                "trim_start_matches" => s.trim_start_matches(&pat),
                "trim_end_matches" => s.trim_end_matches(&pat),
                // trim_matches only takes chars in real Rust
                _ => s.trim_matches(pat.chars().next().unwrap_or(' ')),
            };
            Value::str(out.to_string())
        }
        "cmp" => make_ordering((***s).cmp(arg_str(0).as_str())),
        // A String or a Cow that already owns its data, into_owned returns self.
        "into_owned" | "into_string" => Value::Str(s.clone()),
        _ => {
            if let Some(colored) = color_method(s, name) {
                colored
            } else {
                return generic_method(&Value::Str(s.clone()), name, args);
            }
        }
    })
}

/// The `colored` crate as string methods. Returns the styled text as a plain
/// string carrying ANSI codes, so chaining and printing both work. Honors the
/// crate's own NO_COLOR and terminal detection.
pub(super) fn color_method(s: &str, name: &str) -> Option<Value> {
    use colored::Colorize;
    let out = match name {
        "red" => s.red(),
        "green" => s.green(),
        "yellow" => s.yellow(),
        "blue" => s.blue(),
        "magenta" | "purple" => s.magenta(),
        "cyan" => s.cyan(),
        "white" => s.white(),
        "black" => s.black(),
        "bright_red" => s.bright_red(),
        "bright_green" => s.bright_green(),
        "bright_yellow" => s.bright_yellow(),
        "bright_blue" => s.bright_blue(),
        "bright_cyan" => s.bright_cyan(),
        "on_red" => s.on_red(),
        "on_green" => s.on_green(),
        "on_blue" => s.on_blue(),
        "bold" => s.bold(),
        "dimmed" => s.dimmed(),
        "italic" => s.italic(),
        "underline" => s.underline(),
        "reversed" => s.reversed(),
        "clear" | "normal" => s.normal(),
        _ => return None,
    };
    Some(Value::str(out.to_string()))
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
    let as_f = || match recv {
        Value::Int(i) => *i as f64,
        Value::Float(f) => *f,
        _ => 0.0,
    };
    Ok(match (recv, name) {
        (_, "to_string") => Value::str(recv.display()),
        (_, "clone") => recv.clone(),
        (Value::Int(i), "as_i64" | "as_u64" | "as_i128" | "as_usize") => {
            Value::some(Value::Int(*i))
        }
        (_, "as_f64") => Value::some(Value::Float(as_f())),
        // A number is not these serde types, so the accessor is None.
        (_, "as_str" | "as_bool" | "as_array" | "as_object") => Value::none(),
        (Value::Int(i), "abs") => Value::Int(i.abs()),
        (Value::Float(f), "abs") => Value::Float(f.abs()),
        (Value::Int(i), "pow") => Value::Int(i.pow(int_arg(args, 0)? as u32)),
        (Value::Float(f), "powi") => Value::Float(f.powi(int_arg(args, 0)? as i32)),
        (Value::Float(f), "powf") => Value::Float(f.powf(float_arg(args, 0)?)),
        (Value::Float(_), "sqrt") => Value::Float(as_f().sqrt()),
        (Value::Float(f), "floor") => Value::Float(f.floor()),
        (Value::Float(f), "ceil") => Value::Float(f.ceil()),
        (Value::Float(f), "round") => Value::Float(f.round()),
        (Value::Float(f), "is_sign_positive") => Value::Bool(f.is_sign_positive()),
        (Value::Int(a), "min") => Value::Int((*a).min(int_arg(args, 0)?)),
        (Value::Int(a), "max") => Value::Int((*a).max(int_arg(args, 0)?)),
        (Value::Int(a), "clamp") => Value::Int((*a).clamp(int_arg(args, 0)?, int_arg(args, 1)?)),
        (Value::Float(a), "clamp") => {
            Value::Float((*a).clamp(float_arg(args, 0)?, float_arg(args, 1)?))
        }
        (Value::Float(a), "min") => Value::Float((*a).min(float_arg(args, 0)?)),
        (Value::Float(a), "max") => Value::Float((*a).max(float_arg(args, 0)?)),
        (Value::Int(a), "is_multiple_of") => Value::Bool(a % int_arg(args, 0)? == 0),
        (Value::Int(a), "saturating_sub") => Value::Int(a.saturating_sub(int_arg(args, 0)?)),
        (Value::Int(a), "saturating_add") => Value::Int(a.saturating_add(int_arg(args, 0)?)),
        (Value::Int(a), "saturating_mul") => Value::Int(a.saturating_mul(int_arg(args, 0)?)),
        (Value::Int(a), "cmp") => make_ordering(a.cmp(&int_arg(args, 0)?)),
        (_, "partial_cmp") => Value::some(make_ordering(
            as_f()
                .partial_cmp(&float_arg(args, 0)?)
                .unwrap_or(std::cmp::Ordering::Equal),
        )),
        _ => bail!("unknown numeric method `{name}`"),
    })
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
        B::Unwrap => return inner.ok_or_else(|| anyhow!("called unwrap on a None value")),
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
                    "called unwrap on an Err value: {}",
                    inner.map(|v| v.display()).unwrap_or_default()
                );
            }
        }
        "unwrap_err" => {
            if is_ok {
                bail!(
                    "called unwrap_err on an Ok value: {}",
                    inner.map(|v| v.display()).unwrap_or_default()
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

pub(super) fn float_arg(args: &[Value], i: usize) -> Result<f64> {
    match args.get(i) {
        Some(Value::Float(f)) => Ok(*f),
        Some(Value::Int(n)) => Ok(*n as f64),
        _ => bail!("expected a float argument"),
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

/// The result of a `char` method, in a form either engine can turn into its own
/// value type. Keeps the classification table in one place.
pub(super) enum CharOut {
    Bool(bool),
    Char(char),
    Str(String),
}

/// The `char` classification and conversion methods, shared by both engines so
/// a script sees the same set whichever one runs it.
pub(super) fn char_method(ch: char, name: &str) -> Option<CharOut> {
    let b = |v: bool| Some(CharOut::Bool(v));
    match name {
        "is_ascii_digit" => b(ch.is_ascii_digit()),
        "is_ascii_alphabetic" => b(ch.is_ascii_alphabetic()),
        "is_ascii_alphanumeric" => b(ch.is_ascii_alphanumeric()),
        "is_ascii_uppercase" => b(ch.is_ascii_uppercase()),
        "is_ascii_lowercase" => b(ch.is_ascii_lowercase()),
        "is_ascii_whitespace" => b(ch.is_ascii_whitespace()),
        "is_ascii_punctuation" => b(ch.is_ascii_punctuation()),
        "is_ascii_hexdigit" => b(ch.is_ascii_hexdigit()),
        "is_ascii" => b(ch.is_ascii()),
        "is_alphabetic" => b(ch.is_alphabetic()),
        "is_alphanumeric" => b(ch.is_alphanumeric()),
        "is_numeric" => b(ch.is_numeric()),
        "is_whitespace" => b(ch.is_whitespace()),
        "is_uppercase" => b(ch.is_uppercase()),
        "is_lowercase" => b(ch.is_lowercase()),
        "to_ascii_uppercase" => Some(CharOut::Char(ch.to_ascii_uppercase())),
        "to_ascii_lowercase" => Some(CharOut::Char(ch.to_ascii_lowercase())),
        // These yield an iterator in real Rust, but a script only ever renders
        // or collects it, so the string it would produce is handed back.
        "to_uppercase" => Some(CharOut::Str(ch.to_uppercase().to_string())),
        "to_lowercase" => Some(CharOut::Str(ch.to_lowercase().to_string())),
        _ => None,
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
