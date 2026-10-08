//! `FromStr` of the bridge types, the real impl of each crate. `T::from_str(s)` calls it by path,
//! and `s.parse::<T>()` compiles to the same path, see `compile_parse_bridged`.

use std::fmt::{Debug, Display};
use std::net::{IpAddr, SocketAddr};

use anyhow::{Result, anyhow};
use jsonwebtoken::Algorithm;
use regex::Regex;
use reqwest::StatusCode;

use super::bytecode::PathId;
use super::enum_def::ALGORITHM;
use super::json_bridge::{json_to_pvalue, parse_error};
use super::json_paths::bridge_serde_json;
use super::native::Native;
use super::regex_bridge::make_regex;
use super::std_bridge::make_path;
use super::value::Value;

fn parsed<T, E: Display + Debug>(result: Result<T, E>, value: impl FnOnce(T) -> Value) -> Value {
    match result {
        Ok(ok) => Value::ok(value(ok)),
        Err(e) => Value::err(parse_error(&e)),
    }
}

/// A toml tree in the shape `toml::from_str` with no target type gives.
fn toml_tree(tree: &impl serde::Serialize) -> Result<Value> {
    Ok(json_to_pvalue(serde_json::to_value(tree)?))
}

/// `None` when the id is no `from_str` of this file. The chrono types are in `chrono_bridge`,
/// their values are built there.
pub(super) fn from_str_call(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    let text = || args.first().map(Value::display).unwrap_or_default();
    Ok(Some(match id {
        PathId::TableFromStr => match text().parse::<toml::Table>() {
            Ok(table) => Value::ok(toml_tree(&table)?),
            Err(e) => Value::err(parse_error(&e)),
        },
        PathId::TomlValueFromStr => match text().parse::<toml::Value>() {
            Ok(value) => Value::ok(toml_tree(&value)?),
            Err(e) => Value::err(parse_error(&e)),
        },
        PathId::SerdeJsonValueFromStr => bridge_serde_json(PathId::SerdeJsonFromStr, args)?,
        PathId::IpAddrFromStr => parsed(text().parse::<IpAddr>(), |ip| Native::IpAddr(ip).wrap()),
        PathId::SocketAddrFromStr => parsed(text().parse::<SocketAddr>(), |addr| {
            Native::SocketAddr(addr).wrap()
        }),
        // the error type is `Infallible`, so there is no `Err` to build
        PathId::PathBufFromStr => Value::ok(make_path(text())),
        PathId::RegexFromStr => {
            let pattern = text();
            parsed(pattern.parse::<Regex>(), |compiled| {
                make_regex(compiled, &pattern)
            })
        }
        PathId::StatusCodeFromStr => parsed(text().parse::<StatusCode>(), |status| {
            Value::struct_of(
                "StatusCode",
                [("code".into(), Value::Int(i64::from(status.as_u16())))],
            )
        }),
        PathId::AlgorithmFromStr => match text().parse::<Algorithm>() {
            Ok(algorithm) => {
                let name = format!("{algorithm:?}");
                let variant = Value::enum_named(&ALGORITHM, &name, Vec::new())
                    .ok_or_else(|| anyhow!("`Algorithm::{name}` is not bridged"))?;
                Value::ok(variant)
            }
            Err(e) => Value::err(parse_error(&e)),
        },
        _ => return Ok(None),
    }))
}
