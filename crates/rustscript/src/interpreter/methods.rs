//! Builtin methods on strings, Option, Result, entries, and the generic
//! any-receiver names, from the
//! `methods.rs`. Same semantics, `Arc` model.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use super::bridge::VArgs;
use super::bytecode::{BuiltinId, MethodName, ScalarTy};
use super::iterator;
use super::ops::compare_values;
use super::shared::{self, CharOut, Parsed, StrOut, usize_i64};
use super::value::{StructData, Value, ValueRef};

/// `std::cmp::Ordering` as the enum value scripts match on.
pub(super) fn make_ordering(o: std::cmp::Ordering) -> Value {
    let variant = match o {
        std::cmp::Ordering::Less => "Less",
        std::cmp::Ordering::Equal => "Equal",
        std::cmp::Ordering::Greater => "Greater",
    };
    Value::Enum {
        enum_name: Arc::from("Ordering"),
        variant: Arc::from(variant),
        data: Arc::from(Vec::new()),
    }
}

pub(super) fn ordering_from_value(v: &Value) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering::{Equal, Greater, Less};
    match v {
        Value::Enum {
            enum_name, variant, ..
        } if &**enum_name == "Ordering" => match &**variant {
            "Less" => Some(Less),
            "Equal" => Some(Equal),
            "Greater" => Some(Greater),
            _ => None,
        },
        _ => None,
    }
}

/// `map.entry(k).or_insert_with(Vec::new).push(x)` accumulates in place.
///
/// The insert forms answer a reference into the map, because in real Rust
/// they answer `&mut V` and `*map.entry(k).or_insert(0) += x` writes through
/// it. A plain clone would drop that write.
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
            m.lock().entry(key.clone()).or_insert(default);
            Value::Ref(Arc::new(ValueRef::map_entry(m.clone(), key)))
        }
        "or_default" => {
            m.lock()
                .entry(key.clone())
                .or_insert_with(|| Value::vec(Vec::new()));
            Value::Ref(Arc::new(ValueRef::map_entry(m.clone(), key)))
        }
        "key" => key.to_value(),
        _ => bail!("unknown method `{name}` on Entry"),
    })
}

/// The `serde_json` `is_*` family, answered from the shared table.
pub(super) fn json_type_test(recv: &Value, name: &str) -> Option<Value> {
    shared::json_type_test(json_kind(recv), name).map(Value::Bool)
}

/// The `serde_json` methods that apply to a whole `Value` whatever shape it
/// turned out to be, so they are answered before the per type dispatch. See
/// each arm for what it answers and why.
pub(super) fn json_value_method(recv: &Value, name: &str, args: &[Value]) -> Option<Value> {
    if name == "get" {
        return match recv {
            Value::Str(_) if matches!(args.first(), Some(Value::Range { .. })) => None,
            Value::Str(_)
            | Value::Int(_)
            | Value::IntW(..)
            | Value::Float(_)
            | Value::F32(_)
            | Value::Bool(_)
            | Value::Unit => Some(Value::none()),
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
                .and_then(|key| map.lock().get(&key).cloned()),
            Value::Vec(items) => shared::json_pointer_index(&token)
                .and_then(|index| items.lock().get(index).cloned()),
            _ => None,
        };
        match next {
            Some(value) => here = value,
            None => return Some(Value::none()),
        }
    }
    Some(Value::some(here))
}

/// The json shape of a runtime value.
pub(super) fn json_kind(recv: &Value) -> shared::JsonKind {
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
        // Values are structurally typed here, so a conversion that only changes
        // the static type is a no-op. A receiver with a real conversion, an
        // OsString into a PathBuf for example, handles `into` in its own
        // bridge before this.
        (_, "clone" | "into") => Ok(recv.clone()),
        (_, "to_string") => Ok(Value::str(recv.display())),
        (Value::Char(ch), name) if let Some(out) = shared::char_method(*ch, name, &VArgs(args)) => {
            Ok(match out? {
                CharOut::Bool(v) => Value::Bool(v),
                CharOut::Char(c) => Value::Char(c),
                CharOut::Str(s) => Value::str(s),
                CharOut::OptU32(Some(digit)) => Value::some(Value::int_of_width(
                    i128::from(digit),
                    super::numeric::IntWidth::U32,
                )),
                CharOut::OptU32(None) => Value::none(),
            })
        }
        (Value::Bool(b), "as_bool") => Ok(Value::some(Value::Bool(*b))),
        // `then_some(v)` yields that value, not a placeholder.
        (Value::Bool(b), "then_some") => Ok(if *b {
            Value::some(args.first().cloned().unwrap_or(Value::Unit))
        } else {
            Value::none()
        }),
        (Value::Vec(v), "as_array") => Ok(Value::some(Value::vec(v.lock().clone()))),
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

pub(super) fn str_method(s: &Arc<str>, method: &MethodName, args: &[Value]) -> Result<Value> {
    use BuiltinId as B;
    let arg_str = |i: usize| -> String { args.get(i).map(Value::display).unwrap_or_default() };
    Ok(match method.id {
        B::Len => shared::usize_value(s.len()),
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
                    usize_i64(s.len())
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
        B::Chars => iterator::chars(s.clone()),
        B::Lines => iterator::lines(s.clone()),
        B::Split => split_value(s, args.first()),
        B::SplitWhitespace => iterator::split_whitespace(s.clone()),
        B::Count => Value::Int(usize_i64(s.chars().count())),
        B::Parse => parse_value(s, method.scalar.as_ref()),
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
        Some(ScalarTy::Map(_)) => Value::map(),
        Some(ScalarTy::Set(_)) => Value::set(),
        // `Other` is a type this model does not describe, so it is no better
        // informed than no type at all and keeps the same fallback.
        Some(ScalarTy::Str | ScalarTy::Other) | None => Value::str(String::new()),
    }
}

/// Materialize the neutral parse answer as a runtime value.
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

pub(super) fn str_method_slow(s: &Arc<str>, name: &str, args: &[Value]) -> Result<Value> {
    if let Some(out) = shared::str_core(s, name, &VArgs(args))? {
        return Ok(str_out(s, out));
    }
    if let Some(text) = shared::color_core(s, name) {
        return Ok(Value::str(text));
    }
    match name {
        // The lazy iterator form of the byte walk.
        "bytes" => Ok(iterator::bytes(s.clone())),
        _ => generic_method(&Value::Str(s.clone()), name, args),
    }
}

/// Turn a neutral string core answer into a runtime value. `Keep`
/// clones the `Arc`, so handing the receiver back stays a refcount bump.
fn str_out(s: &Arc<str>, out: StrOut) -> Value {
    match out {
        StrOut::Bool(b) => Value::Bool(b),
        StrOut::USize(n) => shared::usize_value(n),
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
            .ok_or_else(|| anyhow!("{}", args.first().map(Value::display).unwrap_or_default()))?,
        // There is no runtime type here, so the payload type's Default cannot
        // be built beyond what the call site wrote down.
        "unwrap_or_default" => inner.unwrap_or_else(|| default_of(method.scalar.as_ref())),
        "as_ref" | "as_deref" | "take" | "as_mut" => recv.clone(),
        // Iterating an Option yields its payload or nothing, as a vec so the
        // chain's `collect`, `rev`, and friends compose on it.
        "into_iter" | "iter" => Value::vec(inner.into_iter().collect()),
        // A json null parses to None here, so a serde lookup into a value that
        // turned out to be null is None rather than an unknown method error.
        "get" => Value::none(),
        "ok_or" | "context" => match inner {
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
        // The interpreter holds no references, so a reference view is the value.
        "clone" | "as_ref" | "as_mut" | "as_deref" | "as_deref_mut" => recv.clone(),
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
            }
            inner.unwrap_or(Value::Unit)
        }
        "expect" => {
            if is_ok {
                inner.unwrap_or(Value::Unit)
            } else {
                bail!("{}", args.first().map(Value::display).unwrap_or_default());
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
        // Iterating a Result yields the payload or nothing, like Option.
        "into_iter" | "iter" => {
            if is_ok {
                Value::vec(inner.into_iter().collect())
            } else {
                Value::vec(Vec::new())
            }
        }
        "context" | "with_context" => {
            if is_ok {
                Value::ok(inner.unwrap_or(Value::Unit))
            } else {
                let ctx = args.first().map(Value::display).unwrap_or_default();
                let cause = inner.map(|v| v.display()).unwrap_or_default();
                Value::err(Value::str(format!("{ctx}\nCaused by: {cause}")))
            }
        }
        _ => return generic_method(recv, method.text.as_str(), args),
    })
}

/// `str::split` with either a string pattern or a set of chars. A char array
/// like `['-', '_']` splits on any of them, which a plain string pattern would
/// otherwise match only as the literal sequence.
pub(super) fn split_value(s: &Arc<str>, pattern: Option<&Value>) -> Value {
    if let Some(Value::Vec(items)) = pattern {
        let chars: Vec<char> = items
            .lock()
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
