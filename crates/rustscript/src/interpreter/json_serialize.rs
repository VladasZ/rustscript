//! Runtime values to `serde_json` values and back, `rename`, `skip_serializing_if` and `flatten`
//! applied to structs.

use std::sync::Arc;

use anyhow::{Result, bail};
use parking_lot::Mutex;

use super::bytecode::JsonMask;
use super::enum_def::{EnumKind, OK, SOME};
use super::native::Native;
use super::serde_types::enum_to_json;
use super::value::{List, Value};

fn is_none(value: &Value) -> bool {
    matches!(value, Value::Enum { def, variant, .. } if def.kind == EnumKind::Option && *variant != SOME)
}

/// For the toml and yaml bridges. Null maps to None like the json parser.
pub(super) fn json_to_pvalue(v: serde_json::Value) -> Value {
    use serde_json::Value as JsonValue;
    match v {
        JsonValue::Null => Value::none(),
        JsonValue::Bool(b) => Value::Bool(b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(u) = n.as_u64() {
                Value::int_of_width(i128::from(u), super::numeric::IntWidth::U64)
            } else {
                Value::Float(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        JsonValue::String(s) => Value::str(s),
        JsonValue::Array(items) => Value::vec(items.into_iter().map(json_to_pvalue).collect()),
        JsonValue::Object(map) => {
            let mut out = super::value::MapStore::default();
            for (k, v) in map {
                if let Some(key) = Value::str(k).into_key() {
                    out.insert(key, json_to_pvalue(v));
                }
            }
            Value::map_of(out)
        }
    }
}

pub(super) fn pvalue_to_json(v: &Value) -> Result<serde_json::Value> {
    use serde_json::Value as JsonValue;
    Ok(match v {
        Value::Unit => JsonValue::Null,
        Value::Bool(b) => JsonValue::Bool(*b),
        Value::Int(i) => JsonValue::Number(serde_json::Number::from(*i)),
        Value::IntW(bits, w) => {
            let value = w.decode(*bits);
            match i64::try_from(value) {
                Ok(small) => JsonValue::Number(serde_json::Number::from(small)),
                Err(_) => JsonValue::Number(serde_json::Number::from(
                    u64::try_from(value).expect("width-tagged value fits u64"),
                )),
            }
        }
        // a 128 bit integer is a number only while it fits the json range
        Value::Big(raw, w) => {
            let as_i64 = if *w == super::numeric::IntWidth::U128 {
                i64::try_from(raw.cast_unsigned()).map_err(|_| ())
            } else {
                i64::try_from(*raw).map_err(|_| ())
            };
            match as_i64 {
                Ok(small) => JsonValue::Number(serde_json::Number::from(small)),
                Err(()) => bail!("128-bit integer does not fit a json number"),
            }
        }
        Value::Float(f) => {
            serde_json::Number::from_f64(*f).map_or(JsonValue::Null, JsonValue::Number)
        }
        Value::F32(f) => {
            serde_json::Number::from_f64(f64::from(*f)).map_or(JsonValue::Null, JsonValue::Number)
        }
        Value::Char(c) => JsonValue::String(c.to_string()),
        Value::Str(s) => JsonValue::String(s.to_string()),
        Value::Vec(items) | Value::Tuple(items) => JsonValue::Array(
            items
                .lock()
                .iter()
                .map(pvalue_to_json)
                .collect::<Result<_>>()?,
        ),
        // serde writes a set as a list
        Value::Map(map, super::value::MapKind::Set) => JsonValue::Array(
            map.lock()
                .keys()
                .map(|k| pvalue_to_json(&k.to_value()))
                .collect::<Result<_>>()?,
        ),
        Value::Map(map, _) => {
            let mut obj = serde_json::Map::default();
            for (k, val) in map.lock().iter() {
                obj.insert(k.to_value().display(), pvalue_to_json(val)?);
            }
            JsonValue::Object(obj)
        }
        Value::Struct(s) => struct_to_json(s)?,
        Value::Enum { def, variant, data } if def.user => {
            super::serde_types::user_enum_to_json(def, *variant, &data.lock())?
        }
        Value::Enum { def, variant, data } => {
            let payload = data.lock().clone();
            if def.kind == EnumKind::Option {
                if *variant == SOME {
                    pvalue_to_json(&payload[0])?
                } else {
                    JsonValue::Null
                }
            } else if payload.is_empty() {
                JsonValue::String(def.variant_name(*variant).to_string())
            } else {
                let mut obj = serde_json::Map::default();
                obj.insert(
                    def.variant_name(*variant).to_string(),
                    JsonValue::Array(payload.iter().map(pvalue_to_json).collect::<Result<_>>()?),
                );
                JsonValue::Object(obj)
            }
        }
        Value::Range { .. } => bail!("cannot serialize a range to json"),
        Value::Closure(_) => bail!("cannot serialize a closure to json"),
        // serde serializes cells by content
        Value::Cell(_, slot) => {
            let inner = slot.lock().clone();
            pvalue_to_json(&inner)?
        }
        Value::Ref(reference) => {
            let Some(value) = reference.get() else {
                bail!("cannot serialize a dangling reference to json");
            };
            pvalue_to_json(&value)?
        }
        Value::Native(n) => bail!("cannot serialize a {} to json", n.lock().type_name()),
    })
}

/// A struct as a json object, `rename`, `skip_serializing_if` and `flatten` applied. A struct
/// variant goes inside its enum representation.
fn struct_to_json(s: &super::value::StructData) -> Result<serde_json::Value> {
    use serde_json::Value as JsonValue;
    let mut obj = serde_json::Map::default();
    let values = s.values.lock();
    for (slot, (field, val)) in s.shape.fields.iter().zip(values.iter()).enumerate() {
        let attrs = s.shape.serde.get(slot).copied().unwrap_or_default();
        if attrs.skip_none && is_none(val) {
            continue;
        }
        if attrs.flatten {
            match pvalue_to_json(val)? {
                JsonValue::Object(inner) => obj.extend(inner),
                // a flattened `None` adds no keys
                JsonValue::Null => {}
                _ => bail!("can only flatten structs and maps, field `{field}`"),
            }
            continue;
        }
        let key = s
            .shape
            .renames
            .get(slot)
            .and_then(Option::as_ref)
            .unwrap_or(field);
        obj.insert(key.to_string(), pvalue_to_json(val)?);
    }
    Ok(match &s.shape.variant {
        Some((def, index)) => enum_to_json(def, *index, Some(JsonValue::Object(obj)))?,
        None => JsonValue::Object(obj),
    })
}

/// The value with every part the mask marks turned into a real `serde_json::Value`, so it prints
/// as json, see `FmtSpec::json`. The value itself is left as it is.
pub(super) fn mark_json(v: &Value, mask: &JsonMask) -> Value {
    if let Value::Ref(reference) = v
        && let Some(inner) = reference.get()
    {
        return mark_json(&inner, mask);
    }
    let first = |data: &List| data.lock().first().cloned().unwrap_or(Value::Unit);
    match (mask, v) {
        (JsonMask::Here, _) => match pvalue_to_json(v) {
            Ok(json) => Native::Json(json).wrap(),
            Err(_) => v.clone(),
        },
        (JsonMask::Items(m), Value::Vec(items)) => {
            Value::vec(items.lock().iter().map(|x| mark_json(x, m)).collect())
        }
        (JsonMask::Values(m), Value::Map(map, kind)) => {
            let mut out = map.lock().clone();
            for x in out.values_mut() {
                *x = mark_json(x, m);
            }
            Value::Map(Arc::new(Mutex::new(out)), *kind)
        }
        (JsonMask::Parts(parts), Value::Tuple(items)) => {
            let mut items = items.lock().clone();
            for (key, m) in parts {
                if let Ok(i) = key.parse::<usize>()
                    && let Some(x) = items.get_mut(i)
                {
                    *x = mark_json(x, m);
                }
            }
            Value::tuple(items)
        }
        (JsonMask::Parts(parts), Value::Struct(s)) => {
            let mut values = s.values.lock().clone();
            for (key, m) in parts {
                if let Some(x) = s.shape.slot(key).and_then(|i| values.get_mut(i)) {
                    *x = mark_json(x, m);
                }
            }
            Value::structure(s.shape.clone(), values)
        }
        (JsonMask::Some(m), Value::Enum { def, variant, data })
            if def.kind == EnumKind::Option && *variant == SOME =>
        {
            Value::some(mark_json(&first(data), m))
        }
        (JsonMask::Result(ok, err), Value::Enum { def, variant, data })
            if def.kind == EnumKind::Result =>
        {
            match (*variant == OK, ok, err) {
                (true, Some(m), _) => Value::ok(mark_json(&first(data), m)),
                (false, _, Some(m)) => Value::err(mark_json(&first(data), m)),
                _ => v.clone(),
            }
        }
        _ => v.clone(),
    }
}
