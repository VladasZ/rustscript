//! The `serde_json` free functions a script calls by path.

use anyhow::{Result, bail};

use super::bridge::arg;
use super::bytecode::PathId;
use super::json_bridge::{json_to_pvalue, parse_json, pvalue_to_json};
use super::value::Value;

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
                Err(e) => Ok(Value::err(Value::str(e.to_string()))),
            }
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
        _ => bail!("unsupported serde_json function `{id}`"),
    }
}
