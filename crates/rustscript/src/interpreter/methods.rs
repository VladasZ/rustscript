//! Builtin methods on strings, Option, Result, entries, and the generic
//! any-receiver names, from the
//! `methods.rs`. Same semantics, `Arc` model.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use super::bridge::VArgs;
use super::bytecode::{BuiltinId as B, MethodName, ScalarTy};
use super::enum_def::{EQUAL, EnumKind, GREATER, LESS, OK, ORDERING, SOME};
use super::iterator;
use super::ops::compare_values;
use super::shared::{self, CharOut, Parsed, StrOut, usize_i64};
use super::value::{Map, MapKey, RsStr, Value, ValueRef};

/// `std::cmp::Ordering` as the enum value scripts match on. A comparator
/// sort builds one per comparison, so the three values are built once and
/// cloned. A unit variant's payload list stays empty forever, which makes
/// the shared storage safe.
pub(super) fn make_ordering(o: std::cmp::Ordering) -> Value {
    use std::sync::LazyLock;
    static ORD_LESS: LazyLock<Value> =
        LazyLock::new(|| Value::enum_of(&ORDERING, LESS, Vec::new()));
    static ORD_EQUAL: LazyLock<Value> =
        LazyLock::new(|| Value::enum_of(&ORDERING, EQUAL, Vec::new()));
    static ORD_GREATER: LazyLock<Value> =
        LazyLock::new(|| Value::enum_of(&ORDERING, GREATER, Vec::new()));
    match o {
        std::cmp::Ordering::Less => ORD_LESS.clone(),
        std::cmp::Ordering::Equal => ORD_EQUAL.clone(),
        std::cmp::Ordering::Greater => ORD_GREATER.clone(),
    }
}

pub(super) fn ordering_from_value(v: &Value) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering::{Equal, Greater, Less};
    match v {
        Value::Enum { def, variant, .. } if def.kind == EnumKind::Ordering => match *variant {
            LESS => Some(Less),
            EQUAL => Some(Equal),
            GREATER => Some(Greater),
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
pub(super) fn entry_method(
    m: &Map,
    key: &MapKey,
    method: &MethodName,
    args: &[Value],
) -> Result<Value> {
    Ok(match method.id {
        B::OrInsert => {
            let default = args.first().cloned().unwrap_or(Value::Unit);
            m.lock().entry(key.clone()).or_insert(default);
            Value::Ref(Arc::new(ValueRef::map_entry(m.clone(), key.clone())))
        }
        B::OrDefault => {
            m.lock()
                .entry(key.clone())
                .or_insert_with(|| Value::vec(Vec::new()));
            Value::Ref(Arc::new(ValueRef::map_entry(m.clone(), key.clone())))
        }
        B::Key => key.to_value(),
        _ => bail!("unknown method `{}` on Entry", method.text),
    })
}

/// The `serde_json` `is_*` family, answered from the shared table.
pub(super) fn json_type_test(recv: &Value, method: &MethodName) -> Option<Value> {
    shared::json_type_test(json_kind(recv), method.id).map(Value::Bool)
}

/// The `serde_json` methods that apply to a whole `Value` whatever shape it
/// turned out to be, so they are answered before the per type dispatch. See
/// each arm for what it answers and why.
pub(super) fn json_value_method(
    recv: &Value,
    method: &MethodName,
    args: &[Value],
) -> Option<Value> {
    if method.id == B::Get {
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
    if !matches!(method.id, B::Pointer | B::PointerMut) {
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
    // `pointer_mut` answers a borrow into the tree, so a mutation through it
    // must reach the tree and the mutation split never applies.
    if method.id == B::PointerMut {
        return Some(Value::some(Value::Ref(Arc::new(ValueRef::borrowed(here)))));
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

pub(super) fn generic_method(recv: &Value, method: &MethodName, args: &[Value]) -> Result<Value> {
    match (recv, method.id) {
        // Values are structurally typed here, so a conversion that only changes
        // the static type is a no-op. A receiver with a real conversion, an
        // OsString into a PathBuf for example, handles `into` in its own
        // bridge before this.
        (_, B::Clone | B::Into) => Ok(recv.clone()),
        (_, B::ToString) => Ok(Value::str(recv.display())),
        (Value::Char(ch), id) if let Some(out) = shared::char_method(*ch, id, &VArgs(args)) => {
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
        (Value::Bool(b), B::AsBool) => Ok(Value::some(Value::Bool(*b))),
        // `then_some(v)` yields that value, not a placeholder.
        (Value::Bool(b), B::ThenSome) => Ok(if *b {
            Value::some(args.first().cloned().unwrap_or(Value::Unit))
        } else {
            Value::none()
        }),
        (Value::Vec(v), B::AsArray) => Ok(Value::some(Value::vec(v.lock().clone()))),
        // `_mut` accessors answer a borrow of the receiver's own storage,
        // so the mutation split never applies to what they hand back.
        (Value::Vec(v), B::AsArrayMut) => Ok(Value::some(Value::Ref(Arc::new(
            ValueRef::borrowed(Value::Vec(v.clone())),
        )))),
        // Serde accessors on a value that is not the matching type, for example
        // as_str on Null, are None rather than an error.
        (_, id) if shared::json_accessor(id) => Ok(Value::none()),
        // `Ord` is derived for every type built out of ordered parts, so these
        // work on an Option or a tuple as much as on a number. This is the
        // last resort dispatch, so a receiver with its own `max` or `min`,
        // a Vec or an integer, never reaches here.
        (_, B::Max | B::Min | B::Cmp) if args.len() == 1 => {
            let other = &args[0];
            let ordering = compare_values(recv, other)?;
            Ok(match method.id {
                B::Cmp => make_ordering(ordering),
                B::Max if ordering.is_ge() => recv.clone(),
                B::Min if ordering.is_le() => recv.clone(),
                _ => other.clone(),
            })
        }
        // `std::cmp::Ordering` chaining and tests. `then_with` takes a
        // closure and answers from the higher order path instead.
        (Value::Enum { def, .. }, _)
            if def.kind == EnumKind::Ordering && ordering_from_value(recv).is_some() =>
        {
            let ordering = ordering_from_value(recv).expect("checked by the guard");
            ordering_method(ordering, method, args)
        }
        // An enum names itself, so an unknown method on an Option says Option
        // and not the bare word enum. A struct names itself the same way.
        (Value::Enum { def, .. }, _) => {
            bail!("unknown method `{}` on {}", method.text, def.name)
        }
        (Value::Struct(s), _) => {
            bail!("unknown method `{}` on struct `{}`", method.text, s.name())
        }
        _ => bail!("unknown method `{}` on {}", method.text, recv.type_name()),
    }
}

/// The value-taking `std::cmp::Ordering` methods.
fn ordering_method(o: std::cmp::Ordering, method: &MethodName, args: &[Value]) -> Result<Value> {
    let chained = || args.first().cloned().unwrap_or_else(|| make_ordering(o));
    Ok(match method.id {
        B::Then => {
            if o == std::cmp::Ordering::Equal {
                chained()
            } else {
                make_ordering(o)
            }
        }
        B::Reverse => make_ordering(o.reverse()),
        B::IsLt => Value::Bool(o.is_lt()),
        B::IsLe => Value::Bool(o.is_le()),
        B::IsGt => Value::Bool(o.is_gt()),
        B::IsGe => Value::Bool(o.is_ge()),
        B::IsEq => Value::Bool(o.is_eq()),
        B::IsNe => Value::Bool(o.is_ne()),
        _ => bail!("unknown method `{}` on Ordering", method.text),
    })
}

/// A `str::get(range)` slice, None when the bounds are out of range or land
/// inside a character, exactly what the real method answers.
fn str_slice(s: &str, start: i64, end: i64) -> Option<&str> {
    let start = usize::try_from(start).ok()?;
    let end = usize::try_from(end).ok()?;
    s.get(start..end)
}

pub(super) fn str_method(s: &RsStr, method: &MethodName, args: &[Value]) -> Result<Value> {
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
        _ => return str_method_slow(s, method, args),
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

pub(super) fn str_method_slow(s: &RsStr, method: &MethodName, args: &[Value]) -> Result<Value> {
    if let Some(out) = shared::str_core(s, method.id, &VArgs(args))? {
        return Ok(str_out(s, out));
    }
    if let Some(text) = shared::color_core(s, method.id) {
        return Ok(Value::str(text));
    }
    match method.id {
        // The lazy iterator form of the byte walk.
        B::Bytes => Ok(iterator::bytes(s.clone())),
        _ => generic_method(&Value::Str(s.clone()), method, args),
    }
}

/// Turn a neutral string core answer into a runtime value. `Keep`
/// clones the `Arc`, so handing the receiver back stays a refcount bump.
fn str_out(s: &RsStr, out: StrOut) -> Value {
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
    // The hot accessors dispatch on the id before the variant is even looked
    // at, and the payload is cloned only on the paths that hand it out.
    if let B::Clone | B::Copied | B::Cloned = method.id {
        return Ok(recv.clone());
    }
    let (is_some, inner) = match recv {
        Value::Enum { variant, data, .. } => (*variant == SOME, data.lock().first().cloned()),
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
    Ok(match method.id {
        B::IsSome => Value::Bool(is_some),
        B::IsNone => Value::Bool(!is_some),
        B::Expect => inner
            .ok_or_else(|| anyhow!("{}", args.first().map(Value::display).unwrap_or_default()))?,
        // There is no runtime type here, so the payload type's Default cannot
        // be built beyond what the call site wrote down.
        B::UnwrapOrDefault => inner.unwrap_or_else(|| default_of(method.scalar.as_ref())),
        B::AsRef | B::AsDeref | B::Take | B::AsMut => recv.clone(),
        // Iterating an Option yields its payload or nothing, as a vec so the
        // chain's `collect`, `rev`, and friends compose on it.
        B::IntoIter | B::Iter => Value::vec(inner.into_iter().collect()),
        // A json null parses to None here, so a serde lookup into a value that
        // turned out to be null is None rather than an unknown method error.
        B::Get => Value::none(),
        B::OkOr | B::Context => match inner {
            Some(v) => Value::ok(v),
            None => Value::err(args.first().cloned().unwrap_or(Value::Unit)),
        },
        _ => return generic_method(recv, method, args),
    })
}

pub(super) fn res_method(recv: &Value, method: &MethodName, args: &[Value]) -> Result<Value> {
    let (is_ok, inner) = match recv {
        Value::Enum { variant, data, .. } => (*variant == OK, data.lock().first().cloned()),
        _ => unreachable!(),
    };
    Ok(match method.id {
        B::IsOk => Value::Bool(is_ok),
        B::IsErr => Value::Bool(!is_ok),
        // The interpreter holds no references, so a reference view is the value.
        B::Clone | B::AsRef | B::AsMut | B::AsDeref | B::AsDerefMut => recv.clone(),
        B::Unwrap => {
            if is_ok {
                inner.unwrap_or(Value::Unit)
            } else {
                bail!(
                    "called `Result::unwrap()` on an `Err` value: {}",
                    inner.map(|v| v.debug()).unwrap_or_default()
                );
            }
        }
        B::UnwrapErr => {
            if is_ok {
                bail!(
                    "called `Result::unwrap_err()` on an `Ok` value: {}",
                    inner.map(|v| v.debug()).unwrap_or_default()
                );
            }
            inner.unwrap_or(Value::Unit)
        }
        B::Expect => {
            if is_ok {
                inner.unwrap_or(Value::Unit)
            } else {
                bail!("{}", args.first().map(Value::display).unwrap_or_default());
            }
        }
        B::UnwrapOr => {
            if is_ok {
                inner.unwrap_or(Value::Unit)
            } else {
                args.first().cloned().unwrap_or(Value::Unit)
            }
        }
        // The Ok payload type, from wherever the call site stated it, exactly
        // as Option::unwrap_or_default above.
        B::UnwrapOrDefault => {
            if is_ok {
                inner.unwrap_or_else(|| default_of(method.scalar.as_ref()))
            } else {
                default_of(method.scalar.as_ref())
            }
        }
        B::Ok => {
            if is_ok {
                Value::some(inner.unwrap_or(Value::Unit))
            } else {
                Value::none()
            }
        }
        B::Err => {
            if is_ok {
                Value::none()
            } else {
                Value::some(inner.unwrap_or(Value::Unit))
            }
        }
        // Iterating a Result yields the payload or nothing, like Option.
        B::IntoIter | B::Iter => {
            if is_ok {
                Value::vec(inner.into_iter().collect())
            } else {
                Value::vec(Vec::new())
            }
        }
        B::Context | B::WithContext => {
            if is_ok {
                Value::ok(inner.unwrap_or(Value::Unit))
            } else {
                let ctx = args.first().map(Value::display).unwrap_or_default();
                let cause = inner.map(|v| v.display()).unwrap_or_default();
                Value::err(Value::str(format!("{ctx}\nCaused by: {cause}")))
            }
        }
        _ => return generic_method(recv, method, args),
    })
}

/// `str::split` with either a string pattern or a set of chars. A char array
/// like `['-', '_']` splits on any of them, which a plain string pattern would
/// otherwise match only as the literal sequence.
pub(super) fn split_value(s: &RsStr, pattern: Option<&Value>) -> Value {
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
