//! The `serde_json` free functions a script calls by path.

use anyhow::{Result, bail};

use super::bridge::arg;
use super::bytecode::PathId;
use super::json_bridge::{json_to_pvalue, parse_error, parse_json, pvalue_to_json};
use super::native_methods::value_to_bytes;
use super::value::Value;

/// The text of the bytes `from_slice` reads. Bytes that are not UTF-8 give the error the real
/// parse gives for them.
pub(super) fn slice_text(args: &[Value]) -> Result<String, Value> {
    let bytes = value_to_bytes(args.first());
    String::from_utf8(bytes).map_err(|e| {
        match serde_json::from_slice::<serde::de::IgnoredAny>(e.as_bytes()) {
            Err(parse) => parse_error(&parse),
            Ok(_) => parse_error(&e),
        }
    })
}

/// The dynamic path, `from_str` with no type plus `to_string` and `to_string_pretty`.
pub(super) fn bridge_serde_json(id: PathId, args: &[Value]) -> Result<Value> {
    match id {
        PathId::SerdeJsonFromStr => {
            let owned;
            let s: &str = match args.first() {
                Some(Value::Str(s)) => s,
                Some(other) => {
                    owned = other.display();
                    &owned
                }
                None => bail!("from_str needs a string"),
            };
            match parse_json(s) {
                Ok(v) => Ok(Value::ok(v)),
                Err(e) => Ok(Value::err(parse_error(&e))),
            }
        }
        PathId::SerdeJsonFromSlice => Ok(match slice_text(args) {
            Ok(text) => match parse_json(&text) {
                Ok(v) => Value::ok(v),
                Err(e) => Value::err(parse_error(&e)),
            },
            Err(e) => Value::err(e),
        }),
        PathId::SerdeJsonToVec | PathId::SerdeJsonToVecPretty => {
            let j = pvalue_to_json(&arg(args, 0)?)?;
            let bytes = if id == PathId::SerdeJsonToVecPretty {
                serde_json::to_vec_pretty(&j)?
            } else {
                serde_json::to_vec(&j)?
            };
            Ok(Value::ok(Value::vec(
                bytes.into_iter().map(Value::byte).collect(),
            )))
        }
        PathId::SerdeJsonToString | PathId::SerdeJsonToStringPretty => {
            let v = arg(args, 0)?;
            let j = pvalue_to_json(&v)?;
            let s = if id == PathId::SerdeJsonToStringPretty {
                serde_json::to_string_pretty(&j)?
            } else {
                serde_json::to_string(&j)?
            };
            Ok(Value::ok(Value::str(s)))
        }
        PathId::SerdeJsonToValue => {
            let v = arg(args, 0)?;
            Ok(Value::ok(json_to_pvalue(pvalue_to_json(&v)?)))
        }
        // a json value is already a runtime value, the call site coerces it into the target type
        PathId::SerdeJsonFromValue => Ok(Value::ok(arg(args, 0)?)),
        _ => bail!("unsupported serde_json function `{id}`"),
    }
}
