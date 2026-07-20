//! The Windows registry, backed by the winreg crate.
//!
//! A `RegKey` is stored as plain struct fields, the root HKEY plus the subkey
//! path and the access flags, and the real key is opened on demand for each
//! call. That matches how `Regex` and `Command` are handled, and keeps the live
//! handle out of the `Native` enum, so no other module needs a cfg.
//!
//! Registry types map onto script values by shape. A DWORD or QWORD reads back
//! as an int, a string as a string, binary as a vec of ints, and a multi string
//! as a vec of strings. Writes pick the type the same way, so an int writes a
//! DWORD and a vec of ints writes binary. That covers every value the setup
//! scripts touch, including the binary scancode map and the string valued mouse
//! and accessibility settings.
//!
//! On a non-Windows host every entry point returns a plain error instead of
//! being absent, so a script that reaches registry code by mistake says why.

use std::rc::Rc;

use anyhow::Result;

use super::std_bridge::as_i64;
use super::value::{StructData, Value};

/// The registry value types. `RegType` is a real enum in winreg, so it is
/// mirrored as an enum value and not an int, and `{:?}` prints the bare variant
/// name exactly like the compiled crate does.
const REG_TYPES: [&str; 7] = [
    "REG_NONE",
    "REG_SZ",
    "REG_EXPAND_SZ",
    "REG_BINARY",
    "REG_DWORD",
    "REG_MULTI_SZ",
    "REG_QWORD",
];

fn unit_enum(enum_name: &str, variant: &str) -> Value {
    Value::Enum {
        enum_name: Rc::from(enum_name),
        variant: Rc::from(variant),
        data: Rc::from([]),
    }
}

/// Recognize `HKEY_*` roots, `KEY_*` access flags, and `RegType` variants as
/// path constants.
pub(super) fn winreg_const(name: &str) -> Option<Value> {
    if REG_TYPES.contains(&name) {
        return Some(unit_enum("RegType", name));
    }
    let n = match name {
        "HKEY_CLASSES_ROOT" => 0x8000_0000_u32,
        "HKEY_CURRENT_USER" => 0x8000_0001,
        "HKEY_LOCAL_MACHINE" => 0x8000_0002,
        "HKEY_USERS" => 0x8000_0003,
        "HKEY_CURRENT_CONFIG" => 0x8000_0005,
        "KEY_QUERY_VALUE" => 0x0001,
        "KEY_SET_VALUE" => 0x0002,
        "KEY_CREATE_SUB_KEY" => 0x0004,
        "KEY_ENUMERATE_SUB_KEYS" => 0x0008,
        "KEY_READ" => 0x0002_0019,
        "KEY_WRITE" => 0x0002_0006,
        "KEY_ALL_ACCESS" => 0x000F_003F,
        "KEY_WOW64_64KEY" => 0x0100,
        "KEY_WOW64_32KEY" => 0x0200,
        _ => return None,
    };
    Some(Value::Int(i64::from(n)))
}

/// Build the script side `RegKey` value. `predef` takes one of the HKEY roots.
pub(super) fn predef(args: &[Value]) -> Value {
    let root = args.first().and_then(as_i64).unwrap_or(0);
    key_value(root, "", i64::from(0x000F_003F_u32))
}

fn key_value(root: i64, path: &str, flags: i64) -> Value {
    Value::struct_of(
        "RegKey",
        [
            ("root".into(), Value::Int(root)),
            ("path".into(), Value::str(path)),
            ("flags".into(), Value::Int(flags)),
        ],
    )
}

#[cfg(windows)]
mod imp {
    use std::borrow::Cow;

    use anyhow::{Result, bail};
    use winreg::RegKey;
    use winreg::enums::RegType;
    use winreg::types::{FromRegValue, ToRegValue};

    use super::super::value::Value;
    use super::{as_i64, key_value};
    use crate::interpreter::value::StructData;

    fn field_str(s: &StructData, name: &str) -> String {
        s.get(name).map(|v| v.display()).unwrap_or_default()
    }

    fn field_i64(s: &StructData, name: &str) -> i64 {
        s.get(name).as_ref().and_then(as_i64).unwrap_or_default()
    }

    /// Join a parent subkey path with a child, tolerating either side being empty.
    fn join(parent: &str, child: &str) -> String {
        match (parent.is_empty(), child.is_empty()) {
            (true, _) => child.to_string(),
            (_, true) => parent.to_string(),
            _ => format!("{}\\{}", parent.trim_end_matches('\\'), child),
        }
    }

    fn root_key(root: i64) -> RegKey {
        RegKey::predef(root as isize as winreg::HKEY)
    }

    fn open(s: &StructData) -> std::io::Result<RegKey> {
        let path = field_str(s, "path");
        let flags = field_i64(s, "flags") as u32;
        let root = root_key(field_i64(s, "root"));
        if path.is_empty() {
            // The predefined root itself, there is nothing to open below it.
            return Ok(root);
        }
        root.open_subkey_with_flags(&path, flags)
    }

    /// Turn a live registry value into the script value that matches its type.
    fn read(v: &winreg::RegValue) -> Value {
        match v.vtype {
            // A DWORD is four bytes and a QWORD is eight, and each decoder
            // rejects the other width, so they cannot share an arm.
            RegType::REG_DWORD => {
                Value::Int(u32::from_reg_value(v).map(i64::from).unwrap_or_default())
            }
            RegType::REG_QWORD => {
                Value::Int(u64::from_reg_value(v).map(|n| n as i64).unwrap_or_default())
            }
            RegType::REG_SZ | RegType::REG_EXPAND_SZ => {
                Value::str(String::from_reg_value(v).unwrap_or_default())
            }
            RegType::REG_MULTI_SZ => Value::vec(
                Vec::<String>::from_reg_value(v)
                    .unwrap_or_default()
                    .into_iter()
                    .map(Value::str)
                    .collect(),
            ),
            _ => Value::vec(v.bytes.iter().map(|b| Value::Int(i64::from(*b))).collect()),
        }
    }

    /// Pick the registry type from the shape of the script value. An int that
    /// does not fit a DWORD widens to a QWORD rather than silently truncating.
    ///
    /// `RegValue` borrows its bytes, and every source here is a local, so the
    /// result is copied into an owned buffer before it leaves the function.
    fn write(v: &Value) -> Result<winreg::RegValue<'static>> {
        match v {
            Value::Int(n) => {
                if let Ok(small) = u32::try_from(*n) {
                    Ok(own(small.to_reg_value()))
                } else {
                    let wide = *n as u64;
                    Ok(own(wide.to_reg_value()))
                }
            }
            Value::Vec(items) => {
                let items = items.borrow();
                if items.iter().all(|i| matches!(i, Value::Str(_))) && !items.is_empty() {
                    let strings: Vec<String> = items.iter().map(Value::display).collect();
                    return Ok(own(strings.to_reg_value()));
                }
                let mut bytes = Vec::with_capacity(items.len());
                for i in items.iter() {
                    let Some(n) = as_i64(i) else {
                        bail!("a binary registry value takes a vec of byte ints");
                    };
                    bytes.push(u8::try_from(n.rem_euclid(256)).unwrap_or_default());
                }
                Ok(winreg::RegValue {
                    bytes: Cow::Owned(bytes),
                    vtype: RegType::REG_BINARY,
                })
            }
            other => {
                let text = other.display();
                Ok(own(text.to_reg_value()))
            }
        }
    }

    fn own(v: winreg::RegValue<'_>) -> winreg::RegValue<'static> {
        winreg::RegValue {
            bytes: Cow::Owned(v.bytes.into_owned()),
            vtype: v.vtype,
        }
    }

    fn type_name(t: &RegType) -> &'static str {
        match t {
            RegType::REG_NONE => "REG_NONE",
            RegType::REG_SZ => "REG_SZ",
            RegType::REG_EXPAND_SZ => "REG_EXPAND_SZ",
            RegType::REG_DWORD => "REG_DWORD",
            RegType::REG_MULTI_SZ => "REG_MULTI_SZ",
            RegType::REG_QWORD => "REG_QWORD",
            _ => "REG_BINARY",
        }
    }

    fn type_from_name(name: &str) -> RegType {
        match name {
            "REG_NONE" => RegType::REG_NONE,
            "REG_SZ" => RegType::REG_SZ,
            "REG_EXPAND_SZ" => RegType::REG_EXPAND_SZ,
            "REG_DWORD" => RegType::REG_DWORD,
            "REG_MULTI_SZ" => RegType::REG_MULTI_SZ,
            "REG_QWORD" => RegType::REG_QWORD,
            _ => RegType::REG_BINARY,
        }
    }

    /// The script side mirror of `winreg::RegValue`, the untyped form with the
    /// raw bytes and the value type beside them.
    fn raw_value(v: &winreg::RegValue) -> Value {
        Value::struct_of(
            "RegValue",
            [
                (
                    "bytes".into(),
                    Value::vec(v.bytes.iter().map(|b| Value::Int(i64::from(*b))).collect()),
                ),
                (
                    "vtype".into(),
                    super::unit_enum("RegType", type_name(&v.vtype)),
                ),
            ],
        )
    }

    /// Read a script `RegValue` back into the real one, for `set_raw_value`.
    fn raw_from(v: &Value) -> Result<winreg::RegValue<'static>> {
        let Value::Struct(s) = v else {
            bail!("set_raw_value takes a RegValue");
        };
        let Some(Value::Vec(items)) = s.get("bytes") else {
            bail!("a RegValue needs a bytes field holding a vec of byte ints");
        };
        let items = items.borrow();
        let mut bytes = Vec::with_capacity(items.len());
        for i in items.iter() {
            let Some(n) = as_i64(i) else {
                bail!("a RegValue bytes field takes byte ints");
            };
            bytes.push(u8::try_from(n.rem_euclid(256)).unwrap_or_default());
        }
        let vtype = match s.get("vtype") {
            Some(Value::Enum { variant, .. }) => type_from_name(&variant),
            _ => RegType::REG_BINARY,
        };
        Ok(winreg::RegValue {
            bytes: Cow::Owned(bytes),
            vtype,
        })
    }

    fn io_result(r: std::io::Result<()>) -> Value {
        match r {
            Ok(()) => Value::ok(Value::Unit),
            Err(e) => Value::err(Value::str(e.to_string())),
        }
    }

    pub(super) fn regkey_method(s: &StructData, name: &str, args: &[Value]) -> Result<Value> {
        let arg0 = || args.first().map(Value::display).unwrap_or_default();
        let root = field_i64(s, "root");
        let flags = field_i64(s, "flags");
        let path = field_str(s, "path");

        Ok(match name {
            "open_subkey" | "open_subkey_with_flags" => {
                let want = args.get(1).and_then(as_i64).unwrap_or(flags);
                let full = join(&path, &arg0());
                match root_key(root).open_subkey_with_flags(&full, want as u32) {
                    Ok(_) => Value::ok(key_value(root, &full, want)),
                    Err(e) => Value::err(Value::str(e.to_string())),
                }
            }
            "create_subkey" => {
                let full = join(&path, &arg0());
                match root_key(root).create_subkey(&full) {
                    // winreg hands back the key plus whether it was created or
                    // opened. Scripts destructure the pair like the real crate.
                    Ok((_, disp)) => Value::ok(Value::tuple(vec![
                        key_value(root, &full, flags),
                        super::unit_enum("RegDisposition", &format!("{disp:?}")),
                    ])),
                    Err(e) => Value::err(Value::str(e.to_string())),
                }
            }
            "get_value" => match open(s).and_then(|k| k.get_raw_value(&arg0())) {
                Ok(v) => Value::ok(read(&v)),
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            "set_value" => {
                let Some(v) = args.get(1) else {
                    bail!("set_value takes a name and a value");
                };
                let raw = write(v)?;
                io_result(open(s).and_then(|k| k.set_raw_value(&arg0(), &raw)))
            }
            // The untyped pair. Binary has no typed form in winreg, so a script
            // that writes REG_BINARY goes through these two.
            "get_raw_value" => match open(s).and_then(|k| k.get_raw_value(&arg0())) {
                Ok(v) => Value::ok(raw_value(&v)),
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            "set_raw_value" => {
                let Some(v) = args.get(1) else {
                    bail!("set_raw_value takes a name and a RegValue");
                };
                let raw = raw_from(v)?;
                io_result(open(s).and_then(|k| k.set_raw_value(&arg0(), &raw)))
            }
            "delete_value" => io_result(open(s).and_then(|k| k.delete_value(arg0()))),
            "delete_subkey" => io_result(root_key(root).delete_subkey(join(&path, &arg0()))),
            "delete_subkey_all" => {
                io_result(root_key(root).delete_subkey_all(join(&path, &arg0())))
            }
            "enum_keys" => match open(s) {
                Ok(k) => Value::vec(
                    k.enum_keys()
                        .map(|r| match r {
                            Ok(n) => Value::ok(Value::str(n)),
                            Err(e) => Value::err(Value::str(e.to_string())),
                        })
                        .collect(),
                ),
                Err(e) => Value::vec(vec![Value::err(Value::str(e.to_string()))]),
            },
            "enum_values" => match open(s) {
                Ok(k) => Value::vec(
                    k.enum_values()
                        .map(|r| match r {
                            Ok((n, v)) => Value::ok(Value::tuple(vec![Value::str(n), read(&v)])),
                            Err(e) => Value::err(Value::str(e.to_string())),
                        })
                        .collect(),
                ),
                Err(e) => Value::vec(vec![Value::err(Value::str(e.to_string()))]),
            },
            _ => bail!("unknown method `{name}` on RegKey"),
        })
    }
}

#[cfg(not(windows))]
mod imp {
    use anyhow::{Result, bail};

    use super::super::value::{StructData, Value};

    pub(super) fn regkey_method(_s: &StructData, name: &str, _args: &[Value]) -> Result<Value> {
        bail!("RegKey::{name} is the windows registry, it does not exist on this platform")
    }
}

pub(super) fn winreg_method(s: &StructData, name: &str, args: &[Value]) -> Result<Value> {
    imp::regkey_method(s, name, args)
}
