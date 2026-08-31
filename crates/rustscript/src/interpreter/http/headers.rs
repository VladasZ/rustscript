//! The `HeaderMap` and `HeaderValue` bridge.
//!
//! A map is a struct with one `map` field holding a `Vec` of `(name, value)` tuples, the same shape
//! a response builds, so a repeated header keeps every value in the order it arrived. Names are
//! stored lowercased because `HeaderName` is case insensitive and prints lowercase.

use anyhow::{Result, bail};
use reqwest::header::HeaderName;

use crate::interpreter::bytecode::{BuiltinId, MethodName, PathId};
use crate::interpreter::shared::usize_value;
use crate::interpreter::value::{List, StructData, Value};

pub(super) fn empty_map() -> Value {
    Value::struct_of("HeaderMap", [("map".into(), Value::vec(vec![]))])
}

pub(super) fn header_value(text: impl Into<String>) -> Value {
    Value::struct_of("HeaderValue", [("text".into(), Value::str(text.into()))])
}

/// `HeaderValue::from_static` panics on a bad value, `from_str` gives back the error.
pub(super) fn header_value_call(id: PathId, args: &[Value]) -> Result<Value> {
    let text = args.first().map(Value::display).unwrap_or_default();
    let valid = reqwest::header::HeaderValue::from_str(&text);
    Ok(match id {
        PathId::HeaderValueFromStatic => match valid {
            Ok(_) => header_value(text),
            Err(e) => bail!("invalid header value `{text}`: {e}"),
        },
        _ => match valid {
            Ok(_) => Value::ok(header_value(text)),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
    })
}

/// The text a `HeaderValue` carries, or the plain display of anything else.
fn text_of(v: &Value) -> String {
    if let Value::Struct(s) = v
        && &**s.name() == "HeaderValue"
        && let Some(text) = s.get("text")
    {
        return text.display();
    }
    v.display()
}

/// A lookup takes any name, an invalid one simply matches nothing.
fn lookup_name(args: &[Value]) -> String {
    args.first()
        .map(Value::display)
        .unwrap_or_default()
        .to_lowercase()
}

/// A stored name has to be a real header name, `insert` and `append` panic in compiled Rust when
/// it is not.
fn stored_name(args: &[Value]) -> Result<String> {
    let text = args.first().map(Value::display).unwrap_or_default();
    match HeaderName::try_from(text.as_str()) {
        Ok(name) => Ok(name.as_str().to_string()),
        Err(e) => bail!("invalid header name `{text}`: {e}"),
    }
}

fn entries(s: &StructData) -> Result<List> {
    match s.get("map") {
        Some(Value::Vec(items)) => Ok(items),
        _ => bail!("a HeaderMap without its backing store"),
    }
}

fn pair_parts(item: &Value) -> Option<(String, String)> {
    let Value::Tuple(pair) = item else {
        return None;
    };
    let pair = pair.lock();
    Some((pair[0].display(), pair[1].display()))
}

fn matching(items: &List, name: &str) -> Vec<String> {
    items
        .lock()
        .iter()
        .filter_map(pair_parts)
        .filter(|(k, _)| k == name)
        .map(|(_, v)| v)
        .collect()
}

fn pair(name: &str, value: &str) -> Value {
    Value::tuple(vec![Value::str(name), Value::str(value)])
}

fn opt_header_value(text: Option<String>) -> Value {
    match text {
        Some(text) => Value::some(header_value(text)),
        None => Value::none(),
    }
}

/// The first value under `name` is returned, every value under it is dropped.
fn take_all(items: &List, name: &str) -> Option<String> {
    let mut taken = None;
    items.lock().retain(|item| {
        let Some((k, v)) = pair_parts(item) else {
            return true;
        };
        if k != name {
            return true;
        }
        if taken.is_none() {
            taken = Some(v);
        }
        false
    });
    taken
}

/// `insert` keeps the place the name already had, so the order the map iterates in does not shift.
fn replace(items: &List, name: &str, value: &str) -> Option<String> {
    let at = items
        .lock()
        .iter()
        .position(|item| pair_parts(item).is_some_and(|(k, _)| k == name));
    let taken = take_all(items, name);
    let mut items = items.lock();
    match at {
        Some(at) => items.insert(at, pair(name, value)),
        None => items.push(pair(name, value)),
    }
    taken
}

fn names(items: &List) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (k, _) in items.lock().iter().filter_map(pair_parts) {
        if !out.contains(&k) {
            out.push(k);
        }
    }
    out
}

pub(super) fn header_map_method(
    s: &StructData,
    method: &MethodName,
    args: &[Value],
) -> Result<Value> {
    let items = entries(s)?;
    Ok(match method.id {
        BuiltinId::Get => opt_header_value(matching(&items, &lookup_name(args)).into_iter().next()),
        BuiltinId::GetAll => Value::vec(
            matching(&items, &lookup_name(args))
                .into_iter()
                .map(header_value)
                .collect(),
        ),
        BuiltinId::ContainsKey => Value::Bool(!matching(&items, &lookup_name(args)).is_empty()),
        BuiltinId::Insert => {
            let name = stored_name(args)?;
            let value = args.get(1).map(text_of).unwrap_or_default();
            opt_header_value(replace(&items, &name, &value))
        }
        BuiltinId::Append => {
            let name = stored_name(args)?;
            let value = args.get(1).map(text_of).unwrap_or_default();
            let existed = !matching(&items, &name).is_empty();
            items.lock().push(pair(&name, &value));
            Value::Bool(existed)
        }
        BuiltinId::Remove => opt_header_value(take_all(&items, &lookup_name(args))),
        // a header map counts values, `keys_len` counts names
        BuiltinId::Len => usize_value(items.lock().len()),
        BuiltinId::KeysLen => usize_value(names(&items).len()),
        BuiltinId::IsEmpty => Value::Bool(items.lock().is_empty()),
        BuiltinId::Keys => Value::vec(names(&items).into_iter().map(Value::str).collect()),
        BuiltinId::Values => Value::vec(
            items
                .lock()
                .iter()
                .filter_map(pair_parts)
                .map(|(_, v)| header_value(v))
                .collect(),
        ),
        BuiltinId::Iter => Value::vec(
            items
                .lock()
                .iter()
                .filter_map(pair_parts)
                .map(|(k, v)| Value::tuple(vec![Value::str(k), header_value(v)]))
                .collect(),
        ),
        // the entries live behind an `Arc`, so a shallow copy would alias the original
        BuiltinId::Clone => Value::struct_of(
            "HeaderMap",
            [(
                "map".into(),
                Value::vec(
                    items
                        .lock()
                        .iter()
                        .filter_map(pair_parts)
                        .map(|(k, v)| pair(&k, &v))
                        .collect(),
                ),
            )],
        ),
        _ => bail!("unknown method `{}` on a HeaderMap", method.text),
    })
}

pub(super) fn header_value_method(s: &StructData, method: &MethodName) -> Result<Value> {
    let text = s.get("text").map(|v| v.display()).unwrap_or_default();
    match crate::interpreter::shared::header_value_core(method.id, text) {
        Some(crate::interpreter::shared::HeaderOut::Ok(t)) => Ok(Value::ok(Value::str(t))),
        Some(crate::interpreter::shared::HeaderOut::Text(t)) => Ok(Value::str(t)),
        None => bail!("unknown method `{}` on a HeaderValue", method.text),
    }
}
