//! WMI queries, backed by the wmi crate.
//!
//! A connection is stored as its namespace path and reopened per call, the same
//! shape the registry and service bridges use, so nothing lands in the `Native`
//! enum.
//!
//! A query returns a vec of maps, one per instance, with the property values
//! converted to plain script values. That is what `Get-CimInstance` gives a
//! PowerShell script and it is what the setup scripts read.
//!
//! On a non-Windows host every call returns a plain error saying so.

use anyhow::Result;

use super::value::{StructData, Value};

/// `WMIConnection::new()` defaults to root\cimv2, the namespace almost every
/// query uses. `with_namespace_path` names a different one. Neither takes a
/// `COMLibrary`, the crate initializes COM per thread itself.
pub(super) fn connection(args: &[Value], default_namespace: bool) -> Value {
    let ns = if default_namespace {
        r"root\cimv2".to_string()
    } else {
        args.first().map(Value::display).unwrap_or_default()
    };
    match imp::check_available() {
        Ok(()) => Value::ok(Value::struct_of(
            "WmiConnection",
            [("namespace".into(), Value::str(ns))],
        )),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

pub(super) fn wmi_method(s: &StructData, name: &str, args: &[Value]) -> Result<Value> {
    imp::wmi_method(s, name, args)
}

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use anyhow::{Result, bail};
    use wmi::{Variant, WMIConnection};

    use super::super::value::{Map, MapKey, RStr, StructData, Value};

    /// Nothing to check up front. A namespace that does not exist, or a COM
    /// that will not start, reports itself when the connection is opened.
    pub(super) fn check_available() -> Result<()> {
        Ok(())
    }

    fn connect(namespace: &str) -> Result<WMIConnection> {
        Ok(WMIConnection::with_namespace_path(namespace)?)
    }

    /// Map a WMI variant onto the script value with the same shape.
    ///
    /// Values are returned bare, not wrapped in Some. The map lookup already
    /// hands back an Option, so wrapping here too would make every read a
    /// double Some. A property that is present but null reads as None inside
    /// that outer Some, which still separates "null" from "not there".
    fn from_variant(v: &Variant) -> Value {
        match v {
            Variant::Empty | Variant::Null => Value::none(),
            Variant::String(s) => Value::str(s.clone()),
            Variant::Bool(b) => Value::Bool(*b),
            Variant::I1(n) => Value::Int(i64::from(*n)),
            Variant::I2(n) => Value::Int(i64::from(*n)),
            Variant::I4(n) => Value::Int(i64::from(*n)),
            Variant::I8(n) => Value::Int(*n),
            Variant::UI1(n) => Value::Int(i64::from(*n)),
            Variant::UI2(n) => Value::Int(i64::from(*n)),
            Variant::UI4(n) => Value::Int(i64::from(*n)),
            Variant::UI8(n) => Value::Int(*n as i64),
            Variant::R4(n) => Value::Float(f64::from(*n)),
            Variant::R8(n) => Value::Float(*n),
            Variant::Array(items) => Value::vec(items.iter().map(from_variant).collect()),
            other => Value::str(format!("{other:?}")),
        }
    }

    fn row_to_value(row: &HashMap<String, Variant>) -> Value {
        let mut names: Vec<&String> = row.keys().collect();
        // HashMap order is arbitrary, so sort for a stable script side result.
        names.sort();
        let mut map = Map::default();
        for name in names {
            let Some(v) = row.get(name) else { continue };
            map.insert(MapKey::Str(RStr::new(name.clone())), from_variant(v));
        }
        Value::Map(Rc::new(RefCell::new(map)))
    }

    pub(super) fn wmi_method(s: &StructData, name: &str, args: &[Value]) -> Result<Value> {
        let namespace = s
            .get("namespace")
            .map(|v| v.display())
            .unwrap_or_else(|| r"root\cimv2".to_string());
        Ok(match name {
            "raw_query" | "query" => {
                let q = args.first().map(Value::display).unwrap_or_default();
                match connect(&namespace)
                    .and_then(|c| Ok(c.raw_query::<HashMap<String, Variant>>(&q)?))
                {
                    Ok(rows) => {
                        Value::ok(Value::vec(rows.iter().map(row_to_value).collect()))
                    }
                    Err(e) => Value::err(Value::str(e.to_string())),
                }
            }
            _ => bail!("unknown method `{name}` on WmiConnection"),
        })
    }
}

#[cfg(not(windows))]
mod imp {
    use anyhow::{Result, bail};

    use super::super::value::{StructData, Value};

    pub(super) fn check_available() -> Result<()> {
        bail!("WMI does not exist on this platform")
    }

    pub(super) fn wmi_method(_s: &StructData, name: &str, _args: &[Value]) -> Result<Value> {
        bail!("WmiConnection::{name} is WMI, it does not exist on this platform")
    }
}
