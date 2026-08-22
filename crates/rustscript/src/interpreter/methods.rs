//! Builtin methods on strings, Option, Result, entries and the any receiver names.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use super::bridge::{VArgs, arg};
use super::bytecode::{BuiltinId, MethodName, ScalarTy};
use super::enum_def::{EQUAL, EnumKind, GREATER, LESS, OK, ORDERING, SOME};
use super::iterator;
use super::native::Native;
use super::ops::compare_values;
use super::shared::{self, CharOut, JsonKind, Parsed, StrOut, usize_i64};
use super::value::{Map, MapKey, RsStr, Value, ValueRef};

/// A comparator sort builds one per comparison, so the 3 values are built once and cloned. A unit
/// variant payload stays empty, so sharing is safe.
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

/// The insert forms return a reference into the map, because `*map.entry(k).or_insert(0) += x`
/// writes through it.
pub(super) fn entry_method(
    m: &Map,
    key: &MapKey,
    method: &MethodName,
    args: &[Value],
) -> Result<Value> {
    Ok(match method.id {
        BuiltinId::OrInsert => {
            let default = arg(args, 0)?;
            m.lock().entry(key.clone()).or_insert(default);
            Value::Ref(Arc::new(ValueRef::map_entry(m.clone(), key.clone())))
        }
        BuiltinId::OrDefault => {
            m.lock()
                .entry(key.clone())
                .or_insert_with(|| Value::vec(Vec::new()));
            Value::Ref(Arc::new(ValueRef::map_entry(m.clone(), key.clone())))
        }
        BuiltinId::Key => key.to_value(),
        _ => bail!("unknown method `{}` on Entry", method.text),
    })
}

pub(super) fn json_type_test(recv: &Value, method: &MethodName) -> Option<Value> {
    shared::json_type_test(json_kind(recv), method.id).map(Value::Bool)
}

/// The `serde_json` methods that apply to any shape, handled before the per type dispatch.
pub(super) fn json_value_method(
    recv: &Value,
    method: &MethodName,
    args: &[Value],
) -> Option<Value> {
    if method.id == BuiltinId::Get {
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
    if !matches!(method.id, BuiltinId::Pointer | BuiltinId::PointerMut) {
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
    // `pointer_mut` returns a borrow into the tree, so the mutation split never applies
    if method.id == BuiltinId::PointerMut {
        return Some(Value::some(Value::Ref(Arc::new(ValueRef::borrowed(here)))));
    }
    Some(Value::some(here))
}

pub(super) fn json_kind(recv: &Value) -> shared::JsonKind {
    match recv {
        Value::Map(..) => JsonKind::Object,
        Value::Vec(_) => JsonKind::Array,
        Value::Str(_) => JsonKind::Str,
        Value::Bool(_) => JsonKind::Bool,
        Value::Int(_) | Value::IntW(..) => match recv.int_parts() {
            Some((value, _)) => JsonKind::Int(value),
            None => JsonKind::Other,
        },
        Value::Float(_) | Value::F32(_) => JsonKind::Float,
        // the parser maps a json null to None, Unit counts too
        Value::Unit => JsonKind::Null,
        _ if recv.is_none_value() => JsonKind::Null,
        _ => JsonKind::Other,
    }
}

pub(super) fn generic_method(recv: &Value, method: &MethodName, args: &[Value]) -> Result<Value> {
    match (recv, method.id) {
        // A conversion that only changes the static type is a no-op. A real conversion like
        // `OsString` to `PathBuf` handles `into` in its own bridge first.
        (_, BuiltinId::Clone) => Ok(recv.deep_clone()),
        (_, BuiltinId::Into) => Ok(recv.clone()),
        (_, BuiltinId::ToString) => Ok(Value::str(recv.display())),
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
                CharOut::USize(n) => shared::usize_value(n),
            })
        }
        (Value::Bool(b), BuiltinId::AsBool) => Ok(Value::some(Value::Bool(*b))),
        (Value::Bool(b), BuiltinId::ThenSome) => Ok(if *b {
            Value::some(arg(args, 0)?)
        } else {
            Value::none()
        }),
        (Value::Vec(v), BuiltinId::AsArray) => Ok(Value::some(Value::vec(v.lock().clone()))),
        // `_mut` accessors return a borrow, so the mutation split never applies
        (Value::Vec(v), BuiltinId::AsArrayMut) => Ok(Value::some(Value::Ref(Arc::new(
            ValueRef::borrowed(Value::Vec(v.clone())),
        )))),
        // a wrong type serde accessor is None, not an error
        (_, id) if shared::json_accessor(id) => Ok(Value::none()),
        // `Ord` on any type built from ordered parts. Last resort, a Vec or an integer has its
        // own `max` and never gets here.
        (_, BuiltinId::Max | BuiltinId::Min | BuiltinId::Cmp) if args.len() == 1 => {
            let other = &args[0];
            let ordering = compare_values(recv, other)?;
            Ok(match method.id {
                BuiltinId::Cmp => make_ordering(ordering),
                BuiltinId::Max if ordering.is_ge() => recv.clone(),
                BuiltinId::Min if ordering.is_le() => recv.clone(),
                _ => other.clone(),
            })
        }
        // `then_with` takes a closure and goes through the higher order path
        (Value::Enum { def, .. }, _)
            if def.kind == EnumKind::Ordering && ordering_from_value(recv).is_some() =>
        {
            let ordering = ordering_from_value(recv).expect("checked by the guard");
            ordering_method(ordering, method, args)
        }
        // an unknown method on an Option says Option, not the bare word enum
        (Value::Enum { def, .. }, _) => {
            bail!("unknown method `{}` on {}", method.text, def.name)
        }
        (Value::Struct(s), _) => {
            bail!("unknown method `{}` on struct `{}`", method.text, s.name())
        }
        _ => bail!("unknown method `{}` on {}", method.text, recv.type_name()),
    }
}

fn ordering_method(o: std::cmp::Ordering, method: &MethodName, args: &[Value]) -> Result<Value> {
    let chained = || args.first().cloned().unwrap_or_else(|| make_ordering(o));
    Ok(match method.id {
        BuiltinId::Then => {
            if o == std::cmp::Ordering::Equal {
                chained()
            } else {
                make_ordering(o)
            }
        }
        BuiltinId::Reverse => make_ordering(o.reverse()),
        BuiltinId::IsLt => Value::Bool(o.is_lt()),
        BuiltinId::IsLe => Value::Bool(o.is_le()),
        BuiltinId::IsGt => Value::Bool(o.is_gt()),
        BuiltinId::IsGe => Value::Bool(o.is_ge()),
        BuiltinId::IsEq => Value::Bool(o.is_eq()),
        BuiltinId::IsNe => Value::Bool(o.is_ne()),
        _ => bail!("unknown method `{}` on Ordering", method.text),
    })
}

/// None out of range or inside a character, like the real method.
fn str_slice(s: &str, start: i64, end: i64) -> Option<&str> {
    let start = usize::try_from(start).ok()?;
    let end = usize::try_from(end).ok()?;
    s.get(start..end)
}

pub(super) fn str_method(s: &RsStr, method: &MethodName, args: &[Value]) -> Result<Value> {
    let arg_str = |i: usize| -> String { args.get(i).map(Value::display).unwrap_or_default() };
    Ok(match method.id {
        BuiltinId::Len => shared::usize_value(s.len()),
        BuiltinId::IsEmpty => Value::Bool(s.is_empty()),
        BuiltinId::Clone | BuiltinId::ToString => Value::Str(s.clone()),
        BuiltinId::Trim => Value::str(s.trim().to_string()),
        // Handled by the vm on the register slot. Getting here means the receiver is not
        // addressable and the edit would be lost.
        BuiltinId::Push | BuiltinId::PushStr => {
            // a wrong argument is the error to report first
            let mut scratch = s.clone();
            str_grow(&mut scratch, method.id, &arg(args, 0)?)?;
            bail!("cannot mutate a string through this receiver")
        }
        BuiltinId::Contains => Value::Bool(s.contains(&arg_str(0))),
        BuiltinId::StartsWith => Value::Bool(s.starts_with(&arg_str(0))),
        BuiltinId::EndsWith => Value::Bool(s.ends_with(&arg_str(0))),
        // `str::get(range)`, a json `get` on a string is handled earlier
        BuiltinId::Get => match args.first() {
            Some(Value::Range {
                start,
                end,
                inclusive,
            }) => {
                // an `i64::MAX` end is the open end sentinel for `s.get(3..)`
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
        BuiltinId::Chars => iterator::chars(s.clone()),
        BuiltinId::Lines => iterator::lines(s.clone()),
        BuiltinId::Split => split_value(s, args.first()),
        BuiltinId::SplitWhitespace => iterator::split_whitespace(s.clone()),
        BuiltinId::Count => Value::Int(usize_i64(s.chars().count())),
        BuiltinId::Parse => parse_value(s, method.scalar.as_ref()),
        _ => return str_method_slow(s, method, args),
    })
}

/// The payload type is only known when the call site wrote it. Without it the empty string stays,
/// which `env::var(..).unwrap_or_default()` wants.
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
        // `Other` knows as much as no type at all
        Some(ScalarTy::Str | ScalarTy::Other) | None => Value::str(String::new()),
    }
}

/// The full written type first, the scalar hint after it.
fn payload_default(method: &MethodName) -> Value {
    match &method.default {
        Some(ir) => super::vm_step::build_default(ir),
        None => default_of(method.scalar.as_ref()),
    }
}

pub(super) fn parse_value(text: &str, target: Option<&ScalarTy>) -> Value {
    match shared::parse_core(text, target) {
        Parsed::Int(value, width) => Value::ok(Value::int_of_width(value, width)),
        Parsed::F32(value) => Value::ok(Value::F32(value)),
        Parsed::F64(value) => Value::ok(Value::Float(value)),
        Parsed::Bool(value) => Value::ok(Value::Bool(value)),
        Parsed::Char(value) => Value::ok(Value::Char(value)),
        Parsed::Str(value) => Value::ok(Value::str(value)),
        Parsed::Fail(message) => Value::err(parse_error(message)),
    }
}

/// So `{:?}` shows `ParseIntError { kind: InvalidDigit }` like the real type. A message no std
/// type produces stays a string.
fn parse_error(message: String) -> Value {
    let debug = match message.as_str() {
        "cannot parse integer from empty string" => "ParseIntError { kind: Empty }",
        "invalid digit found in string" => "ParseIntError { kind: InvalidDigit }",
        "number too large to fit in target type" => "ParseIntError { kind: PosOverflow }",
        "number too small to fit in target type" => "ParseIntError { kind: NegOverflow }",
        "number would be zero for non-zero type" => "ParseIntError { kind: Zero }",
        "invalid float literal" => "ParseFloatError { kind: Invalid }",
        "cannot parse float from empty string" => "ParseFloatError { kind: Empty }",
        "provided string was not `true` or `false`" => "ParseBoolError",
        "cannot parse char from empty string" => "ParseCharError { kind: EmptyString }",
        "too many characters in string" => "ParseCharError { kind: TooManyChars }",
        _ => return Value::str(message),
    };
    Native::ParseErr {
        display: message,
        debug: debug.to_string(),
    }
    .wrap()
}

pub(super) fn str_method_slow(s: &RsStr, method: &MethodName, args: &[Value]) -> Result<Value> {
    if let Some(out) = shared::str_core(s, method.id, &VArgs(args))? {
        return Ok(str_out(s, out));
    }
    if let Some(text) = shared::color_core(s, method.id) {
        return Ok(Value::str(text));
    }
    match method.id {
        BuiltinId::Bytes => Ok(iterator::bytes(s.clone())),
        _ => generic_method(&Value::Str(s.clone()), method, args),
    }
}

/// `Keep` clones the `Arc`, a refcount bump.
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
    // the hot accessors dispatch on the id first, the payload is cloned only where it is handed out
    if let BuiltinId::Clone | BuiltinId::Copied | BuiltinId::Cloned = method.id {
        return Ok(recv.deep_clone());
    }
    let (is_some, inner) = match recv {
        Value::Enum { variant, data, .. } => (*variant == SOME, data.lock().first().cloned()),
        _ => unreachable!(),
    };
    match method.id {
        BuiltinId::Unwrap => {
            return inner.ok_or_else(|| anyhow!("called `Option::unwrap()` on a `None` value"));
        }
        BuiltinId::UnwrapOr => {
            return Ok(match inner {
                Some(v) => v,
                None => arg(args, 0)?,
            });
        }
        _ => {}
    }
    Ok(match method.id {
        BuiltinId::IsSome => Value::Bool(is_some),
        BuiltinId::IsNone => Value::Bool(!is_some),
        BuiltinId::Expect => inner
            .ok_or_else(|| anyhow!("{}", args.first().map(Value::display).unwrap_or_default()))?,
        // no runtime type, so only what the call site wrote can build the default
        BuiltinId::UnwrapOrDefault => inner.unwrap_or_else(|| payload_default(method)),
        BuiltinId::AsRef | BuiltinId::AsDeref | BuiltinId::Take | BuiltinId::AsMut => recv.clone(),
        // as a vec so `collect`, `rev` and friends compose on it
        BuiltinId::IntoIter | BuiltinId::Iter => Value::vec(inner.into_iter().collect()),
        // a json null is None, so a serde lookup on it is None and not an unknown method error
        BuiltinId::Get => Value::none(),
        BuiltinId::OkOr | BuiltinId::Context => match inner {
            Some(v) => Value::ok(v),
            None => Value::err(arg(args, 0)?),
        },
        _ => return generic_method(recv, method, args),
    })
}

pub(super) fn res_method(recv: &Value, method: &MethodName, args: &[Value]) -> Result<Value> {
    let (is_ok, inner) = match recv {
        Value::Enum { variant, data, .. } => (*variant == OK, Value::payload(data)?),
        _ => unreachable!(),
    };
    Ok(match method.id {
        BuiltinId::IsOk => Value::Bool(is_ok),
        BuiltinId::IsErr => Value::Bool(!is_ok),
        BuiltinId::And => {
            if is_ok {
                arg(args, 0)?
            } else {
                recv.clone()
            }
        }
        BuiltinId::Or => {
            if is_ok {
                recv.clone()
            } else {
                arg(args, 0)?
            }
        }
        // a reference view is the value
        BuiltinId::Clone
        | BuiltinId::AsRef
        | BuiltinId::AsMut
        | BuiltinId::AsDeref
        | BuiltinId::AsDerefMut => recv.clone(),
        BuiltinId::Unwrap => {
            if is_ok {
                inner
            } else {
                bail!(
                    "called `Result::unwrap()` on an `Err` value: {}",
                    inner.debug()
                );
            }
        }
        BuiltinId::UnwrapErr => {
            if is_ok {
                bail!(
                    "called `Result::unwrap_err()` on an `Ok` value: {}",
                    inner.debug()
                );
            }
            inner
        }
        BuiltinId::Expect => {
            if is_ok {
                inner
            } else {
                bail!("{}", args.first().map(Value::display).unwrap_or_default());
            }
        }
        BuiltinId::UnwrapOr => {
            if is_ok {
                inner
            } else {
                arg(args, 0)?
            }
        }
        // same as `Option::unwrap_or_default` above
        BuiltinId::UnwrapOrDefault => {
            if is_ok {
                inner
            } else {
                payload_default(method)
            }
        }
        BuiltinId::Ok => {
            if is_ok {
                Value::some(inner)
            } else {
                Value::none()
            }
        }
        BuiltinId::Err => {
            if is_ok {
                Value::none()
            } else {
                Value::some(inner)
            }
        }
        BuiltinId::IntoIter | BuiltinId::Iter => {
            if is_ok {
                Value::vec(vec![inner])
            } else {
                Value::vec(Vec::new())
            }
        }
        BuiltinId::Context | BuiltinId::WithContext => {
            if is_ok {
                Value::ok(inner)
            } else {
                let ctx = args.first().map(Value::display).unwrap_or_default();
                let cause = inner.display();
                Value::err(Value::str(format!("{ctx}\nCaused by: {cause}")))
            }
        }
        _ => return generic_method(recv, method, args),
    })
}

/// A `&String` argument arrives as a reference or a shared cell and reads through.
pub(super) fn str_grow(text: &mut RsStr, id: BuiltinId, arg: &Value) -> Result<()> {
    match (id, arg) {
        (BuiltinId::Push, Value::Char(c)) => text.push(*c),
        (BuiltinId::PushStr, Value::Str(other)) => text.push_str(other),
        (BuiltinId::PushStr, Value::Ref(_) | Value::Cell(..)) => text.push_str(&arg.display()),
        (BuiltinId::Push, other) => bail!("push takes a char, not {}", other.type_name()),
        (_, other) => bail!("push_str takes a string, not {}", other.type_name()),
    }
    Ok(())
}

/// A char array like `['-', '_']` splits on any of them.
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
