//! Builtin methods on the plain value types: String, Vec, HashMap,
//! numbers, Option, and Result. Split from `builtins.rs`.

use std::cell::RefCell;
use std::mem::take;
use std::rc::Rc;

use anyhow::{Result, anyhow, bail};

use super::bytecode::{BuiltinId, MethodName, ScalarTy};

use super::value::{Map, MapKind, RStr, StructData, Value};

use super::builtins::*;
use super::ops::compare_values;
use super::shared::{self, Args, CharOut, F32Out, Num, NumOut, Parsed, StrOut};

/// `map.entry(k).or_insert_with(Vec::new).push(x)` accumulates in place.
pub(super) fn entry_method(s: &StructData, name: &str, args: &[Value]) -> Result<Value> {
    let key = s
        .get("key")
        .and_then(|k| k.as_key())
        .ok_or_else(|| anyhow!("invalid entry key"))?;
    let Some(Value::Map(m, _)) = s.get("map") else {
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
/// here, so each one is a type test, answered from the shared table so the
/// parallel engine cannot drift from this one.
pub(super) fn json_type_test(recv: &Value, name: &str) -> Option<Value> {
    shared::json_type_test(json_kind(recv), name).map(Value::Bool)
}

/// The serde_json methods that apply to a whole `Value` whatever shape it
/// turned out to be, so they are answered before the per type dispatch.
///
/// `pointer` and `pointer_mut` are the RFC 6901 lookup, and a pointer that
/// leaves the tree answers None instead of failing. The interpreter shares its
/// containers, so the mut form hands back the same value the plain one does.
///
/// `get` reaches here only on a shape with no lookup of its own, a string, a
/// number or a bool, where serde answers None rather than failing. A map, a
/// vec and an Option each have their own `get` and never reach this. A range
/// argument on a string is `str::get`, a different method, so that one is left
/// alone rather than silently answered None.
pub(super) fn json_value_method(recv: &Value, name: &str, args: &[Value]) -> Option<Value> {
    if name == "get" {
        return match recv {
            Value::Str(_) if matches!(args.first(), Some(Value::Range { .. })) => None,
            Value::Str(_) | Value::Int(_) | Value::IntW(..) | Value::Float(_) | Value::F32(_) => {
                Some(Value::none())
            }
            Value::Bool(_) | Value::Unit => Some(Value::none()),
            _ => None,
        };
    }
    if !matches!(name, "pointer" | "pointer_mut") {
        return None;
    }
    let path = args.first().map(Value::display).unwrap_or_default();
    let Some(tokens) = shared::json_pointer_tokens(&path) else {
        return Some(Value::none());
    };
    let mut here = recv.clone();
    for token in tokens {
        let next = match &here {
            Value::Map(map, _) => Value::str(token)
                .as_key()
                .and_then(|key| map.borrow().get(&key).cloned()),
            Value::Vec(items) => shared::json_pointer_index(&token)
                .and_then(|index| items.borrow().get(index).cloned()),
            _ => None,
        };
        match next {
            Some(value) => here = value,
            None => return Some(Value::none()),
        }
    }
    Some(Value::some(here))
}

/// The json shape of a fast engine value.
fn json_kind(recv: &Value) -> shared::JsonKind {
    use shared::JsonKind as K;
    match recv {
        Value::Map(..) => K::Object,
        Value::Vec(_) => K::Array,
        Value::Str(_) => K::Str,
        Value::Bool(_) => K::Bool,
        Value::Int(_) | Value::IntW(..) => match recv.int_parts() {
            Some((value, _)) => K::Int(value),
            None => K::Other,
        },
        Value::Float(_) | Value::F32(_) => K::Float,
        // The parser maps a json null to None, so that is what is_null has to
        // answer for. Unit counts too, it is the interpreter's own empty value.
        Value::Unit => K::Null,
        _ if recv.is_none_value() => K::Null,
        _ => K::Other,
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
        (Value::Vec(v), "as_array_mut") => Ok(Value::some(Value::Vec(v.clone()))),
        // Serde accessors on a value that is not the matching type, for example
        // as_str on Null, are None rather than an error.
        (_, name) if shared::json_accessor(name) => Ok(Value::none()),
        // `Ord` is derived for every type built out of ordered parts, so these
        // work on an Option or a tuple as much as on a number. This is the
        // last resort dispatch, so a receiver with its own `max` or `min`,
        // a Vec or an integer, never reaches here.
        (_, "max" | "min" | "cmp") if args.len() == 1 => {
            let other = &args[0];
            let ordering = compare_values(recv, other)?;
            Ok(match name {
                "cmp" => make_ordering(ordering),
                "max" if ordering.is_ge() => recv.clone(),
                "min" if ordering.is_le() => recv.clone(),
                _ => other.clone(),
            })
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

/// A `str::get(range)` slice, None when the bounds are out of range or land
/// inside a character, exactly what the real method answers.
fn str_slice(s: &str, start: i64, end: i64) -> Option<&str> {
    let start = usize::try_from(start).ok()?;
    let end = usize::try_from(end).ok()?;
    s.get(start..end)
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
        // `str::get(range)`, the real slice method. A json `get` on a string
        // is answered before the per type dispatch and never reaches here.
        B::Get => match args.first() {
            Some(Value::Range {
                start,
                end,
                inclusive,
            }) => {
                // An i64::MAX end is the open-end sentinel compile_range emits
                // for `s.get(3..)`, read as len like the slicing op does.
                let end = if *end == i64::MAX {
                    s.len() as i64
                } else if *inclusive {
                    end + 1
                } else {
                    *end
                };
                match str_slice(s, *start, end) {
                    Some(part) => Value::some(Value::str(part)),
                    None => Value::none(),
                }
            }
            _ => Value::none(),
        },
        B::Chars => super::iterator::chars(s.clone()),
        B::Lines => super::iterator::lines(s.clone()),
        B::Split => split_value(s, args.first()),
        B::SplitWhitespace => super::iterator::split_whitespace(s.clone()),
        B::Count => Value::Int(s.chars().count() as i64),
        B::Parse => parse_value(s.as_str(), method.scalar.as_ref()),
        _ => return str_method_slow(s, &method.text, args),
    })
}

/// `Default::default()` for the payload type of an `Option` or `Result`.
///
/// The type is only known when the call site wrote it down, as `None::<u64>`
/// does. Without it there is no runtime type to build a default from, so this
/// keeps the empty string the bridge has always answered with, which is what
/// the common `env::var(..).unwrap_or_default()` shape wants.
fn default_of(target: Option<&ScalarTy>) -> Value {
    match target {
        Some(ScalarTy::Int(width)) => Value::int_of_width(0, *width),
        Some(ScalarTy::F32) => Value::F32(0.0),
        Some(ScalarTy::F64) => Value::Float(0.0),
        Some(ScalarTy::Bool) => Value::Bool(false),
        Some(ScalarTy::Char) => Value::Char('\0'),
        Some(ScalarTy::Opt(_)) => Value::none(),
        Some(ScalarTy::List(_)) => Value::vec(Vec::new()),
        // `Other` is a type this model does not describe, so it is no better
        // informed than no type at all and keeps the same fallback.
        Some(ScalarTy::Str | ScalarTy::Other) | None => Value::str(String::new()),
    }
}

/// Materialize the engine neutral parse answer as a fast engine value.
pub(super) fn parse_value(text: &str, target: Option<&ScalarTy>) -> Value {
    match shared::parse_core(text, target) {
        Parsed::Int(value, width) => Value::ok(Value::int_of_width(value, width)),
        Parsed::F32(value) => Value::ok(Value::F32(value)),
        Parsed::F64(value) => Value::ok(Value::Float(value)),
        Parsed::Bool(value) => Value::ok(Value::Bool(value)),
        Parsed::Char(value) => Value::ok(Value::Char(value)),
        Parsed::Str(value) => Value::ok(Value::str(value)),
        Parsed::Fail(message) => Value::err(Value::str(message)),
    }
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
            Some(tagged @ Value::IntW(..)) => tagged.untag_int(),
            _ => None,
        }
    }

    fn float(&self, i: usize) -> Option<f64> {
        match self.0.get(i) {
            Some(Value::Float(f)) => Some(*f),
            Some(Value::F32(f)) => Some(f64::from(*f)),
            Some(Value::Int(n)) => Some(*n as f64),
            Some(tagged @ Value::IntW(..)) => tagged.untag_int().map(|n| n as f64),
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
        B::IterMut => super::iterator::value_iter_mut(v.clone()),
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
        // A json array reads by index, and serde answers None for a key that
        // is not one rather than failing, so a non-integer argument is None.
        B::Get => match args.first().and_then(|a| a.int_parts()) {
            Some((index, _)) => match usize::try_from(index)
                .ok()
                .and_then(|i| v.borrow().get(i).cloned())
            {
                Some(x) => Value::some(x),
                None => Value::none(),
            },
            None => Value::none(),
        },
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
        B::SplitFirst => match v.borrow().split_first() {
            Some((head, rest)) => {
                Value::some(Value::tuple(vec![head.clone(), Value::vec(rest.to_vec())]))
            }
            None => Value::none(),
        },
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
            // `Sum` for floats starts at -0.0 so summing negative zeros keeps
            // the sign, matching std.
            let mut acc_f = -0.0f64;
            let mut is_float = false;
            for x in v.borrow().iter() {
                match &x.bridge_image().unwrap_or_else(|| x.clone()) {
                    Value::Int(i) => {
                        acc_i = acc_i
                            .checked_add(*i)
                            .ok_or_else(|| anyhow!("attempt to add with overflow"))?;
                        // A `sum::<u8>()` overflows at 255, not at `i64::MAX`.
                        if let Some(ScalarTy::Int(width)) = &method.scalar
                            && (i128::from(acc_i) < width.min() || i128::from(acc_i) > width.max())
                        {
                            bail!("attempt to add with overflow");
                        }
                    }
                    Value::Float(f) => {
                        is_float = true;
                        acc_f += f;
                    }
                    _ => bail!("sum needs numbers"),
                }
            }
            // An empty vec carries no element to say whether the sum is an
            // integer or a float one, so a `sum::<f64>()` turbofish is the
            // only thing that can tell them apart.
            let float_target = matches!(method.scalar, Some(ScalarTy::F32 | ScalarTy::F64));
            if is_float || (float_target && acc_i == 0) {
                // The integer side only joins when it carries a value, so it
                // cannot cancel the -0.0 identity with a +0.0.
                let total = if acc_i == 0 {
                    acc_f
                } else {
                    acc_f + acc_i as f64
                };
                if matches!(method.scalar, Some(ScalarTy::F32)) {
                    Value::F32(total as f32)
                } else {
                    Value::Float(total)
                }
            } else if let Some(ScalarTy::Int(width)) = &method.scalar {
                // The sum carries the element width, so a later `!` or shift on
                // it computes at that width, not at i64's.
                Value::int_of_width(i128::from(acc_i), *width)
            } else {
                Value::Int(acc_i)
            }
        }
        B::Product => {
            let mut acc_i = 1i64;
            let mut acc_f = 1f64;
            let mut is_float = false;
            for x in v.borrow().iter() {
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
                Value::Float(acc_f * acc_i as f64)
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
            // `by_ref` lends the iterator out, so whatever the borrow hands on
            // is gone from this one too. A draining view over the same vector
            // is that borrow.
            "by_ref" => super::iterator::draining_iter(v.clone()),
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
            // A lazy iterator argument is drained into a vec before it gets
            // here, see `builtin_method`'s caller. Anything else that is not a
            // vec is an error rather than a silent no-op: extending by nothing
            // and reporting success hides the bug in the caller's data.
            "extend" | "append" | "extend_from_slice" => {
                let Some(Value::Vec(other)) = args.first() else {
                    bail!("`{}` needs something iterable", method.text);
                };
                let appended: Vec<Value> = other.borrow().clone();
                v.borrow_mut().extend(appended);
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
            // Iterators are eager vectors here, so `next` takes the front item
            // and leaves the rest behind. Handing back the first item without
            // removing it read correctly on its own but left the iterator
            // unconsumed, so a following `collect` saw the item again and
            // `split(..).next()` then `collect()` returned the whole text.
            // Taking from the front shifts the rest, which is nothing at the
            // sizes a script splits, and the check gate keeps this off real
            // vectors, where it would not compile anyway.
            "next" => {
                let mut items = v.borrow_mut();
                if items.is_empty() {
                    Value::none()
                } else {
                    Value::some(items.remove(0))
                }
            }
            // With an argument this is `Ord::max` on two whole vecs, which
            // orders them lexicographically and hands one back. Only the
            // no-argument form is the iterator reduction over the elements.
            "max" | "min" => {
                if let Some(other) = args.first() {
                    let recv = Value::Vec(v.clone());
                    let ord = compare_values(&recv, other)?;
                    let take_recv = if method.text == "max" {
                        ord.is_ge()
                    } else {
                        ord.is_le()
                    };
                    return Ok(if take_recv { recv } else { other.clone() });
                }
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
                // The mut accessor has to hand back the same list, not a copy,
                // so a push through it reaches the value it was taken from.
                "as_array_mut" => Value::some(Value::Vec(v.clone())),
                "as_object" | "as_object_mut" => Value::none(),
                // Names that apply to any receiver, `clone` and `into` and the
                // rest, live in one place instead of being repeated per type.
                other => return generic_method(&Value::Vec(v.clone()), other, args),
            },
        },
    })
}

pub(super) fn map_method(
    m: &Rc<RefCell<Map>>,
    kind: MapKind,
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
        B::Clone => Value::Map(Rc::new(RefCell::new(m.borrow().clone())), kind),
        B::Insert => {
            let k = take(&mut args[0])
                .into_key()
                .ok_or_else(|| anyhow!("invalid map key"))?;
            // A set's insert takes only the element and answers whether it
            // was new, a map's takes a value and answers the old one.
            if kind == MapKind::Set {
                let old = m.borrow_mut().insert(k, Value::Unit);
                return Ok(Value::Bool(old.is_none()));
            }
            let val = args.get_mut(1).map(take).unwrap_or(Value::Unit);
            let old = m.borrow_mut().insert(k, val);
            match old {
                Some(v) => Value::some(v),
                None => Value::none(),
            }
        }
        // A set's get answers the stored element itself, not the Unit value
        // that backs it.
        B::Get if kind == MapKind::Set => {
            let arg = args.first().ok_or_else(|| anyhow!("invalid map key"))?;
            let k = arg.key_ref().ok_or_else(|| anyhow!("invalid map key"))?;
            match m.borrow().get_key_value(&k) {
                Some((key, _)) => Value::some(key.to_value()),
                None => Value::none(),
            }
        }
        B::Get => lookup(0, &|v| match v {
            Some(v) => Value::some(v.clone()),
            None => Value::none(),
        })?,
        B::Contains if kind == MapKind::Set => lookup(0, &|v| Value::Bool(v.is_some()))?,
        B::ContainsKey => lookup(0, &|v| Value::Bool(v.is_some()))?,
        B::Remove => {
            let arg = args.first().ok_or_else(|| anyhow!("invalid map key"))?;
            let k = arg.key_ref().ok_or_else(|| anyhow!("invalid map key"))?;
            let removed = m.borrow_mut().shift_remove(&k);
            // A set's remove answers whether the element was there.
            if kind == MapKind::Set {
                return Ok(Value::Bool(removed.is_some()));
            }
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
                ("map".into(), Value::Map(m.clone(), kind)),
                ("key".into(), args.first().cloned().unwrap_or(Value::Unit)),
            ],
        ),
        B::Iter if kind == MapKind::Set => set_items(m),
        B::Iter => map_pairs(m),
        _ => match method.text.as_str() {
            "values_mut" => Value::vec(m.borrow().values().cloned().collect()),
            "drain" if kind == MapKind::Set => set_items(m),
            "drain" => map_pairs(m),
            // A JSON object parsed by the interpreter is a Map, so the
            // serde_json accessors resolve against it here. A Map is Rc shared,
            // so the mut accessor is the same call: what it hands back is the
            // same map, and an insert through it reaches the original value.
            "as_object" | "as_object_mut" => Value::some(Value::Map(m.clone(), kind)),
            "as_array" | "as_array_mut" => Value::none(),
            name => return generic_method(&Value::Map(m.clone(), kind), name, &*args),
        },
    })
}

/// The elements of a set, for `iter` and `into_iter` and `drain`.
fn set_items(m: &Rc<RefCell<Map>>) -> Value {
    Value::vec(m.borrow().keys().map(|k| k.to_value()).collect())
}

pub(super) fn map_pairs(m: &Rc<RefCell<Map>>) -> Value {
    Value::vec(
        m.borrow()
            .iter()
            .map(|(k, v)| Value::tuple(vec![k.to_value(), v.clone()]))
            .collect(),
    )
}

/// Answer a width-aware integer method, or `None` when the receiver is not an
/// integer or the name is not one of them, so dispatch falls through.
pub(super) fn int_method(recv: &Value, name: &str, args: &[Value]) -> Option<Result<Value>> {
    let (value, width) = recv.int_parts()?;
    let mut decoded = Vec::with_capacity(args.len());
    for arg in args {
        // Every argument of these methods is an integer in real Rust, so a
        // non-integer here means this is not the method it looks like.
        decoded.push(arg.int_parts()?.0);
    }
    Some(
        match super::int_methods::int_method(name, width, value, &decoded)? {
            Ok(out) => Ok(int_out(out, width)),
            Err(error) => Err(error),
        },
    )
}

fn int_out(out: super::int_methods::IntOut, width: super::numeric::IntWidth) -> Value {
    use super::int_methods::IntOut;
    match out {
        IntOut::Same(value) => Value::int_of_width(value, width),
        // The counting family answers u32 in real Rust, so the tag has to say
        // so, or `!x.count_ones()` computes in 64 bits and prints -1 where the
        // compiled binary prints 4294967295.
        IntOut::Count(count) => {
            Value::int_of_width(i128::from(count), super::numeric::IntWidth::U32)
        }
        IntOut::Bool(value) => Value::Bool(value),
        IntOut::Checked(Some(value)) => Value::some(Value::int_of_width(value, width)),
        IntOut::Checked(None) => Value::none(),
        IntOut::SomeFloat(value) => Value::some(Value::Float(value)),
        IntOut::Ordering(ordering) => make_ordering(ordering),
        // A byte array is a plain vec of untagged ints, the same shape
        // `as_bytes` and `fs::read` produce, so every bridge that eats bytes
        // eats these too.
        IntOut::Bytes(bytes) => Value::vec(
            bytes
                .into_iter()
                .map(|byte| Value::Int(i64::from(byte)))
                .collect(),
        ),
    }
}

/// Materialize an f32 core answer as a fast engine value. Called before
/// `bridge_image` widens the receiver, so the result keeps the f32 tag.
pub(super) fn f32_method(recv: f32, name: &str, args: &[Value]) -> Result<Option<Value>> {
    Ok(
        shared::f32_core(recv, name, &VArgs(args))?.map(|out| match out {
            F32Out::Val(value) => Value::F32(value),
            F32Out::Bool(value) => Value::Bool(value),
            F32Out::SomeOrdering(ordering) => Value::some(make_ordering(ordering)),
        }),
    )
}

pub(super) fn num_method(recv: &Value, name: &str, args: &[Value]) -> Result<Value> {
    match name {
        "to_string" => return Ok(Value::str(recv.display())),
        // A number never reaches the generic dispatch below, so the conversion
        // that only changes the static type is answered here. `2.into()` for a
        // `serde_json::Number` is the same 2.
        "clone" | "into" => return Ok(recv.clone()),
        _ => {}
    }
    let n = match recv {
        Value::Int(i) => Num::Int(*i),
        Value::Float(f) => Num::Float(*f),
        _ => bail!("unknown method `{name}` on a number"),
    };
    match shared::num_core(n, name, &VArgs(args))? {
        Some(out) => Ok(num_out(out)),
        None => bail!("unknown method `{name}` on a number"),
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
        "unwrap_or_default" => inner.unwrap_or_else(|| default_of(method.scalar.as_ref())),
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
        // The Ok payload type, from wherever the call site stated it, exactly
        // as Option::unwrap_or_default above.
        "unwrap_or_default" => {
            if is_ok {
                inner.unwrap_or_else(|| default_of(method.scalar.as_ref()))
            } else {
                default_of(method.scalar.as_ref())
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
        Value::IntW(..) => match v.bridge_image() {
            Some(Value::Int(i)) => SortKey::Int(i),
            _ => SortKey::Str(v.display()),
        },
        Value::F32(f) => SortKey::Float(f64::from(*f)),
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
