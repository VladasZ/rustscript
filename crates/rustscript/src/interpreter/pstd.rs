//! Bridges for the `std` paths a `#[tokio::main]` script calls: fs, io
//! streams, env, dirs, and numeric conversions. Mirrors `std_bridge.rs` on the
//! `Send + Sync` value model, so a parallel script can walk directories and
//! probe streams the same way a fast-engine script does.

use std::sync::Arc;

use anyhow::{Result, bail};

use super::pvalue::{PStructData, PValue};

/// Native implementations of the supported std subset, dispatched by the last
/// two path segments as `module::func`. Returns None when the path is not
/// covered here, so the caller can try user functions next.
pub(super) fn native_call(module: &str, func: &str, args: &[PValue]) -> Result<Option<PValue>> {
    let s = |i: usize| -> Result<String> {
        match args.get(i) {
            Some(v) => Ok(path_like(v)),
            None => bail!("missing argument {i} for {module}::{func}"),
        }
    };
    Ok(Some(match (module, func) {
        ("fs", "read_to_string") => wrap_io(std::fs::read_to_string(s(0)?)),
        ("fs", "read") => wrap_bytes(std::fs::read(s(0)?)),
        ("fs", "write") => wrap_unit(std::fs::write(s(0)?, s(1)?)),
        ("fs", "create_dir_all") => wrap_unit(std::fs::create_dir_all(s(0)?)),
        ("fs", "read_dir") => match std::fs::read_dir(s(0)?) {
            Ok(rd) => {
                let mut items = Vec::new();
                for e in rd {
                    match e {
                        Ok(entry) => items.push(PValue::ok(make_dir_entry(&entry))),
                        Err(err) => items.push(PValue::err(PValue::str(err.to_string()))),
                    }
                }
                PValue::ok(PValue::vec(items))
            }
            Err(e) => PValue::err(PValue::str(e.to_string())),
        },
        ("fs", "metadata") => match std::fs::metadata(s(0)?) {
            Ok(m) => PValue::ok(make_metadata(&m)),
            Err(e) => PValue::err(PValue::str(e.to_string())),
        },
        ("env", "var") => match std::env::var(s(0)?) {
            Ok(v) => PValue::ok(PValue::str(v)),
            Err(e) => PValue::err(PValue::str(e.to_string())),
        },
        ("io", "stdin" | "stdout" | "stderr") => make_std_stream(func),
        ("dirs", "home_dir") => match dirs::home_dir() {
            Some(p) => PValue::some(make_path(p.display().to_string())),
            None => PValue::none(),
        },
        ("which", "which") => match which::which(s(0)?) {
            Ok(p) => PValue::ok(make_path(p.display().to_string())),
            Err(e) => PValue::err(PValue::str(e.to_string())),
        },
        ("String", "from_utf8_lossy") => PValue::str(bytes_to_string(args.first())),
        // Every integer type parses the same way here, values are untyped ints.
        (
            "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
            | "usize",
            "from_str_radix",
        ) => {
            let text = args.first().map(PValue::display).unwrap_or_default();
            let radix = match args.get(1) {
                Some(PValue::Int(i)) => *i as u32,
                _ => 10,
            };
            match i64::from_str_radix(text.trim(), radix) {
                Ok(n) => PValue::ok(PValue::Int(n)),
                Err(e) => PValue::err(PValue::str(e.to_string())),
            }
        }
        // Numeric `T::from(x)`. Every integer is an i64 here, so a widening
        // conversion just carries the value.
        (
            "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
            | "usize",
            "from",
        ) => PValue::Int(int_from_arg(module, args.first())?),
        ("f32" | "f64", "from") => match args.first() {
            Some(PValue::Float(f)) => PValue::Float(*f),
            Some(PValue::Int(n)) => PValue::Float(*n as f64),
            Some(PValue::Bool(b)) => PValue::Float(if *b { 1.0 } else { 0.0 }),
            _ => bail!("`{module}::from` needs a number"),
        },
        // Fallible `T::try_from(x)`. The value fits when it lands inside the
        // target range, so a narrowing conversion reports overflow with the
        // same message as the real `TryFromIntError`.
        (
            "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
            | "usize",
            "try_from",
        ) => {
            let n = int_from_arg(module, args.first())?;
            if int_fits(module, n) {
                PValue::ok(PValue::Int(n))
            } else {
                PValue::err(PValue::str(
                    "out of range integral type conversion attempted",
                ))
            }
        }
        _ => return Ok(None),
    }))
}

// -- path, directory entry, file type, metadata, streams --------------------

pub(super) fn make_path(s: impl Into<String>) -> PValue {
    PValue::struct_of("Path", [("s".into(), PValue::str(s.into()))])
}

fn make_dir_entry(entry: &std::fs::DirEntry) -> PValue {
    PValue::struct_of(
        "DirEntry",
        [
            (
                "path".into(),
                PValue::str(entry.path().display().to_string()),
            ),
            (
                "name".into(),
                PValue::str(entry.file_name().to_string_lossy().into_owned()),
            ),
        ],
    )
}

fn make_file_type(path: &std::path::Path) -> PValue {
    // DirEntry::file_type does not follow symlinks, so a symlink to a dir
    // reports is_symlink, not is_dir, same as the real std.
    let ft = path.symlink_metadata().map(|m| m.file_type());
    let is =
        |f: &dyn Fn(&std::fs::FileType) -> bool| PValue::Bool(ft.as_ref().map(f).unwrap_or(false));
    PValue::struct_of(
        "FileType",
        [
            ("is_dir".into(), is(&|t| t.is_dir())),
            ("is_file".into(), is(&|t| t.is_file())),
            ("is_symlink".into(), is(&|t| t.is_symlink())),
        ],
    )
}

fn make_metadata(m: &std::fs::Metadata) -> PValue {
    PValue::struct_of(
        "Metadata",
        [
            ("len".into(), PValue::Int(m.len() as i64)),
            ("is_dir".into(), PValue::Bool(m.is_dir())),
            ("is_file".into(), PValue::Bool(m.is_file())),
            ("is_symlink".into(), PValue::Bool(m.is_symlink())),
            ("readonly".into(), PValue::Bool(m.permissions().readonly())),
        ],
    )
}

fn make_std_stream(kind: &str) -> PValue {
    PValue::struct_of("StdStream", [("kind".into(), PValue::str(kind))])
}

pub(super) fn path_method(st: &Arc<PStructData>, m: &str, args: &[PValue]) -> Result<PValue> {
    let s = st.get("s").map(|v| v.display()).unwrap_or_default();
    let p = std::path::Path::new(&s);
    let opt_str = |o: Option<&std::ffi::OsStr>| match o {
        Some(v) => PValue::some(PValue::str(v.to_string_lossy().into_owned())),
        None => PValue::none(),
    };
    Ok(match m {
        "display" | "to_string_lossy" => PValue::str(s.clone()),
        "to_str" => PValue::some(PValue::str(s.clone())),
        "into_string" | "into_os_string" => PValue::ok(PValue::str(s.clone())),
        "to_owned" | "to_path_buf" | "as_path" | "as_os_str" => make_path(s.clone()),
        "is_dir" => PValue::Bool(p.is_dir()),
        "is_file" => PValue::Bool(p.is_file()),
        "is_absolute" => PValue::Bool(p.is_absolute()),
        "exists" => PValue::Bool(p.exists()),
        "file_name" => match p.file_name() {
            Some(n) => PValue::some(make_path(n.to_string_lossy().into_owned())),
            None => PValue::none(),
        },
        "file_stem" => opt_str(p.file_stem()),
        "extension" => opt_str(p.extension()),
        "parent" => match p.parent() {
            Some(par) => PValue::some(make_path(par.display().to_string())),
            None => PValue::none(),
        },
        "ancestors" => PValue::vec(
            p.ancestors()
                .map(|ancestor| make_path(ancestor.display().to_string()))
                .collect(),
        ),
        "join" | "push" => {
            let joined = p.join(args.first().map(PValue::display).unwrap_or_default());
            make_path(joined.display().to_string())
        }
        _ => bail!("method `{m}` on Path is not supported in tokio mode"),
    })
}

pub(super) fn os_string_method(st: &Arc<PStructData>, m: &str) -> Result<PValue> {
    let value = st.get("s").map(|v| v.display()).unwrap_or_default();
    Ok(match m {
        "into" => make_path(value),
        "to_string_lossy" | "to_str" => PValue::str(value),
        "is_empty" => PValue::Bool(value.is_empty()),
        _ => bail!("method `{m}` on OsString is not supported in tokio mode"),
    })
}

pub(super) fn dir_entry_method(st: &Arc<PStructData>, m: &str) -> Result<PValue> {
    let path = st.get("path").map(|v| v.display()).unwrap_or_default();
    Ok(match m {
        "path" => make_path(path),
        "file_name" => make_path(st.get("name").map(|v| v.display()).unwrap_or_default()),
        "file_type" => PValue::ok(make_file_type(std::path::Path::new(&path))),
        _ => bail!("method `{m}` on DirEntry is not supported in tokio mode"),
    })
}

pub(super) fn file_type_method(st: &Arc<PStructData>, m: &str) -> Result<PValue> {
    Ok(match m {
        "is_dir" | "is_file" | "is_symlink" => st.get(m).unwrap_or(PValue::Bool(false)),
        _ => bail!("method `{m}` on FileType is not supported in tokio mode"),
    })
}

pub(super) fn metadata_method(st: &Arc<PStructData>, m: &str) -> Result<PValue> {
    Ok(match m {
        "len" | "is_dir" | "is_file" | "is_symlink" | "readonly" => {
            st.get(m).unwrap_or(PValue::Unit)
        }
        _ => bail!("method `{m}` on Metadata is not supported in tokio mode"),
    })
}

pub(super) fn std_stream_method(st: &Arc<PStructData>, m: &str) -> Result<PValue> {
    use std::io::IsTerminal;
    Ok(match m {
        "is_terminal" => {
            let kind = st.get("kind").map(|v| v.display()).unwrap_or_default();
            PValue::Bool(match kind.as_str() {
                "stdin" => std::io::stdin().is_terminal(),
                "stderr" => std::io::stderr().is_terminal(),
                _ => std::io::stdout().is_terminal(),
            })
        }
        "lock" | "by_ref" => PValue::Struct(st.clone()),
        _ => bail!("method `{m}` on a std stream is not supported in tokio mode"),
    })
}

// -- helpers ----------------------------------------------------------------

/// Turn a value into a path string. A `Path`/`PathBuf`/`OsString` value carries
/// the path in its `s` field; anything else uses its display form.
fn path_like(v: &PValue) -> String {
    match v {
        PValue::Struct(st) if matches!(&**st.name(), "Path" | "PathBuf" | "OsString") => {
            st.get("s").map(|s| s.display()).unwrap_or_default()
        }
        other => other.display(),
    }
}

fn wrap_io(r: std::io::Result<String>) -> PValue {
    match r {
        Ok(s) => PValue::ok(PValue::str(s)),
        Err(e) => PValue::err(PValue::str(e.to_string())),
    }
}

fn wrap_bytes(r: std::io::Result<Vec<u8>>) -> PValue {
    match r {
        Ok(bytes) => PValue::ok(PValue::vec(
            bytes
                .into_iter()
                .map(|b| PValue::Int(i64::from(b)))
                .collect(),
        )),
        Err(e) => PValue::err(PValue::str(e.to_string())),
    }
}

fn wrap_unit(r: std::io::Result<()>) -> PValue {
    match r {
        Ok(()) => PValue::ok(PValue::Unit),
        Err(e) => PValue::err(PValue::str(e.to_string())),
    }
}

fn bytes_to_string(arg: Option<&PValue>) -> String {
    match arg {
        Some(PValue::Str(s)) => s.to_string(),
        Some(PValue::Vec(v)) => {
            let bytes: Vec<u8> = v
                .lock()
                .iter()
                .filter_map(|x| match x {
                    PValue::Int(i) => Some(*i as u8),
                    _ => None,
                })
                .collect();
            String::from_utf8_lossy(&bytes).into_owned()
        }
        _ => String::new(),
    }
}

fn int_from_arg(ty: &str, v: Option<&PValue>) -> Result<i64> {
    match v {
        Some(PValue::Int(n)) => Ok(*n),
        Some(PValue::Bool(b)) => Ok(i64::from(*b)),
        Some(PValue::Char(c)) => Ok(*c as i64),
        _ => bail!("`{ty}` conversion needs an integer"),
    }
}

/// Whether `n` lands inside the target integer type range.
fn int_fits(ty: &str, n: i64) -> bool {
    match ty {
        "i8" => i8::try_from(n).is_ok(),
        "i16" => i16::try_from(n).is_ok(),
        "i32" => i32::try_from(n).is_ok(),
        "u8" => u8::try_from(n).is_ok(),
        "u16" => u16::try_from(n).is_ok(),
        "u32" => u32::try_from(n).is_ok(),
        "u64" | "u128" | "usize" => n >= 0,
        _ => true,
    }
}
