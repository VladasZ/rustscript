//! Bridges for `std` paths.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use super::bytecode::{BuiltinId, MethodName, PathId};
use super::crates_bridge::crate_bridge;
use super::enum_def::{NOT_PRESENT, NOT_UNICODE, VAR_ERROR};
use super::json_bridge::bridge_serde_json;
use super::native::Native;
use super::native_methods;
use super::value::{StructData, Value};

fn fs_native_call(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    let s = |i: usize| -> Result<String> {
        match args.get(i) {
            Some(v) => Ok(path_like(v)),
            None => bail!("missing argument {i} for {id}"),
        }
    };
    Ok(Some(match id {
        PathId::FsReadToString => wrap_io(std::fs::read_to_string(s(0)?)),
        PathId::FsRead => wrap_bytes(std::fs::read(s(0)?)),
        PathId::FsWrite => wrap_unit(std::fs::write(s(0)?, s(1)?)),
        PathId::FsCreateDirAll => wrap_unit(std::fs::create_dir_all(s(0)?)),
        PathId::FsCreateDir => wrap_unit(std::fs::create_dir(s(0)?)),
        PathId::FsRemoveFile => wrap_unit(std::fs::remove_file(s(0)?)),
        PathId::FsRemoveDirAll => wrap_unit(std::fs::remove_dir_all(s(0)?)),
        PathId::FsRemoveDir => wrap_unit(std::fs::remove_dir(s(0)?)),
        PathId::FsCopy => match std::fs::copy(s(0)?, s(1)?) {
            Ok(n) => Value::ok(Value::Int(i64::try_from(n).unwrap_or(i64::MAX))),
            Err(e) => Value::err(super::native::io_error_value(&e)),
        },
        PathId::FsRename => wrap_unit(std::fs::rename(s(0)?, s(1)?)),
        PathId::FsReadDir => match std::fs::read_dir(s(0)?) {
            Ok(rd) => {
                let mut items = Vec::new();
                for e in rd {
                    match e {
                        Ok(entry) => items.push(Value::ok(make_dir_entry(&entry))),
                        Err(err) => items.push(Value::err(super::native::io_error_value(&err))),
                    }
                }
                Value::ok(Value::vec(items))
            }
            Err(e) => Value::err(super::native::io_error_value(&e)),
        },
        PathId::FsCanonicalize => match std::fs::canonicalize(s(0)?) {
            Ok(p) => Value::ok(make_path(p.display().to_string())),
            Err(e) => Value::err(super::native::io_error_value(&e)),
        },
        PathId::FsMetadata => match std::fs::metadata(s(0)?) {
            Ok(m) => Value::ok(make_metadata(&m)),
            Err(e) => Value::err(super::native::io_error_value(&e)),
        },
        PathId::FsSymlinkMetadata => match std::fs::symlink_metadata(s(0)?) {
            Ok(m) => Value::ok(make_metadata(&m)),
            Err(e) => Value::err(super::native::io_error_value(&e)),
        },
        PathId::FsReadLink => match std::fs::read_link(s(0)?) {
            Ok(p) => Value::ok(make_path(p.display().to_string())),
            Err(e) => Value::err(super::native::io_error_value(&e)),
        },
        PathId::FsHardLink => wrap_unit(std::fs::hard_link(s(0)?, s(1)?)),
        // the platform specific names all dispatch to 1 helper, so a cfg gated `use` works on each os
        PathId::FsSymlink | PathId::FsSymlinkFile | PathId::FsSymlinkDir => {
            wrap_unit(make_symlink(&s(0)?, &s(1)?))
        }
        PathId::FsSetPermissions => wrap_unit(set_permissions_impl(
            &s(0)?,
            args.get(1).and_then(perm_mode),
        )),
        _ => return Ok(None),
    }))
}

pub(super) fn native_call(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    if let Some(v) = fs_native_call(id, args)? {
        return Ok(Some(v));
    }
    let s = |i: usize| -> Result<String> {
        match args.get(i) {
            Some(v) => Ok(path_like(v)),
            None => bail!("missing argument {i} for {id}"),
        }
    };
    Ok(Some(match id {
        PathId::SerdeJsonFromStr
        | PathId::SerdeJsonToString
        | PathId::SerdeJsonToStringPretty
        | PathId::SerdeJsonToValue => return bridge_serde_json(id, args).map(Some),
        PathId::EnvArgs => Value::vec(super::script_args().into_iter().map(Value::str).collect()),
        PathId::EnvVar => match std::env::var(s(0)?) {
            Ok(v) => Value::ok(Value::str(v)),
            // so `Err(VarError::NotPresent)` matches and `{e:?}` prints `NotPresent`
            Err(std::env::VarError::NotPresent) => {
                Value::err(Value::enum_of(&VAR_ERROR, NOT_PRESENT, Vec::new()))
            }
            Err(std::env::VarError::NotUnicode(os)) => Value::err(Value::enum_of(
                &VAR_ERROR,
                NOT_UNICODE,
                vec![Value::str(os.to_string_lossy().into_owned())],
            )),
        },
        PathId::EnvCurrentDir => match std::env::current_dir() {
            Ok(p) => Value::ok(make_path(p.display().to_string())),
            Err(e) => Value::err(super::native::io_error_value(&e)),
        },
        PathId::EnvSetVar => {
            // Safety: scripts treat the environment as script wide state.
            unsafe { std::env::set_var(s(0)?, s(1)?) };
            Value::Unit
        }
        PathId::EnvRemoveVar => {
            unsafe { std::env::remove_var(s(0)?) };
            Value::Unit
        }
        PathId::EnvVarOs => match std::env::var_os(s(0)?) {
            Some(v) => Value::some(make_os_string(v.to_string_lossy().into_owned())),
            None => Value::none(),
        },
        PathId::EnvVars | PathId::EnvVarsOs => Value::vec(
            std::env::vars()
                .map(|(k, v)| Value::tuple(vec![Value::str(k), Value::str(v)]))
                .collect(),
        ),
        PathId::EnvSetCurrentDir => wrap_unit(std::env::set_current_dir(s(0)?)),
        PathId::EnvTempDir => make_path(std::env::temp_dir().display().to_string()),
        PathId::ProcessExit => {
            let code = args
                .first()
                .and_then(as_i64)
                .and_then(|c| i32::try_from(c).ok())
                .unwrap_or(0);
            std::process::exit(code);
        }
        PathId::ProcessAbort => std::process::abort(),
        PathId::ProcessId => Value::Int(i64::from(std::process::id())),
        // io
        PathId::IoStdin => make_std_stream(
            "stdin",
            Native::Reader(std::io::BufReader::new(Box::new(std::io::stdin()))),
        ),
        PathId::IoStdout => make_std_stream("stdout", Native::Writer(Box::new(std::io::stdout()))),
        PathId::IoStderr => make_std_stream("stderr", Native::Writer(Box::new(std::io::stderr()))),
        _ => return crate_bridge(id, args),
    }))
}

/// `Windows` needs distinct calls for a file and a dir symlink.
fn make_symlink(src: &str, dst: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(src, dst)
    }
    #[cfg(windows)]
    {
        if std::path::Path::new(src).is_dir() {
            std::os::windows::fs::symlink_dir(src, dst)
        } else {
            std::os::windows::fs::symlink_file(src, dst)
        }
    }
}

fn perm_mode(v: &Value) -> Option<u32> {
    if let Value::Struct(st) = v
        && &**st.name() == "Permissions"
    {
        return st
            .get("mode")
            .and_then(|m| as_i64(&m))
            .and_then(|m| u32::try_from(m).ok());
    }
    None
}

fn set_permissions_impl(path: &str, mode: Option<u32>) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode.unwrap_or(0o644)))
    }
    #[cfg(windows)]
    {
        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_readonly(mode.is_some_and(|mode| mode & 0o222 == 0));
        std::fs::set_permissions(path, permissions)
    }
}

/// A value nested in a struct field or a vec never passed through `bridge_image`, so it still
/// carries its declared width and a plain `Int` match would miss it.
pub(super) fn as_i64(v: &Value) -> Option<i64> {
    let (n, _) = v.int_parts()?;
    i64::try_from(n).ok()
}

/// A `PathBuf` or `OsString` has the text in its `s` field, anything else uses its display form.
pub(super) fn path_like(v: &Value) -> String {
    match v {
        Value::Str(s) => s.to_string(),
        Value::Struct(st) if matches!(&**st.name(), "Path" | "PathBuf" | "OsString") => {
            st.get("s").map(|s| s.display()).unwrap_or_default()
        }
        other => other.display(),
    }
}

/// So `is_terminal` can name its stream.
pub(super) fn make_std_stream(kind: &str, inner: Native) -> Value {
    Value::struct_of(
        "StdStream",
        [
            ("kind".into(), Value::str(kind)),
            ("inner".into(), inner.wrap()),
        ],
    )
}

pub(super) fn std_stream_method(
    s: &Arc<StructData>,
    name: &MethodName,
    args: &mut [Value],
) -> Result<Value> {
    use std::io::IsTerminal;
    if name.id == BuiltinId::IsTerminal {
        let kind = s.get("kind").map(|v| v.display()).unwrap_or_default();
        let tty = match kind.as_str() {
            "stdin" => std::io::stdin().is_terminal(),
            "stderr" => std::io::stderr().is_terminal(),
            _ => std::io::stdout().is_terminal(),
        };
        return Ok(Value::Bool(tty));
    }
    if matches!(name.id, BuiltinId::Lock | BuiltinId::ByRef) {
        return Ok(Value::Struct(s.clone()));
    }
    let inner = match s.get("inner") {
        Some(Value::Native(h)) => h.clone(),
        _ => bail!("std stream lost its handle"),
    };
    match native_methods::native_method(&inner, name, args)? {
        Some(v) => Ok(v),
        None => bail!("unknown method `{name}` on a std stream"),
    }
}

pub(super) fn duration_from_value(v: &Value) -> Option<std::time::Duration> {
    if let Value::Struct(s) = v
        && &**s.name() == "Duration"
    {
        let secs = u64::try_from(field_int(s, "secs")).unwrap_or_default();
        let nanos = u32::try_from(field_int(s, "nanos")).unwrap_or_default();
        return Some(std::time::Duration::new(secs, nanos));
    }
    None
}

pub(super) fn make_duration(d: std::time::Duration) -> Value {
    Value::struct_of(
        "Duration",
        [
            (
                "secs".into(),
                Value::Int(i64::try_from(d.as_secs()).unwrap_or(i64::MAX)),
            ),
            ("nanos".into(), Value::Int(i64::from(d.subsec_nanos()))),
        ],
    )
}

/// The Unix `MetadataExt` fields are gated so the interpreter still builds on `Windows`.
pub(super) fn make_metadata(m: &std::fs::Metadata) -> Value {
    let mut f: Vec<(Arc<str>, Value)> = vec![
        (
            "len".into(),
            Value::Int(i64::try_from(m.len()).unwrap_or(i64::MAX)),
        ),
        ("is_dir".into(), Value::Bool(m.is_dir())),
        ("is_file".into(), Value::Bool(m.is_file())),
        ("is_symlink".into(), Value::Bool(m.is_symlink())),
        ("readonly".into(), Value::Bool(m.permissions().readonly())),
    ];
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;
        f.push(("mode".into(), Value::Int(i64::from(m.permissions().mode()))));
        f.push(("dev".into(), Value::Int(m.dev().cast_signed())));
        f.push(("ino".into(), Value::Int(m.ino().cast_signed())));
        f.push(("uid".into(), Value::Int(i64::from(m.uid()))));
        f.push(("gid".into(), Value::Int(i64::from(m.gid()))));
        f.push(("mtime".into(), Value::Int(m.mtime())));
    }
    if let Ok(t) = m.modified() {
        f.push(("modified".into(), Native::SystemTime(t).wrap()));
    }
    Value::struct_of("Metadata", f)
}

// path, directory entry, and file type

pub(super) fn make_path(s: impl Into<String>) -> Value {
    Value::struct_of("Path", [("s".into(), Value::str(s.into()))])
}

pub(super) fn make_os_string(s: impl Into<String>) -> Value {
    Value::struct_of("OsString", [("s".into(), Value::str(s.into()))])
}

pub(super) fn os_string_method(s: &Arc<StructData>, method: &MethodName) -> Result<Value> {
    let value = s.get("s").map(|value| value.display()).unwrap_or_default();
    Ok(match method.id {
        BuiltinId::Into => make_path(value),
        BuiltinId::ToStringLossy | BuiltinId::ToStr => Value::str(value),
        BuiltinId::IsEmpty => Value::Bool(value.is_empty()),
        _ => bail!("unknown method `{method}` on OsString"),
    })
}

pub(super) fn make_dir_entry(entry: &std::fs::DirEntry) -> Value {
    Value::struct_of(
        "DirEntry",
        [
            (
                "path".into(),
                Value::str(entry.path().display().to_string()),
            ),
            (
                "name".into(),
                Value::str(entry.file_name().to_string_lossy().into_owned()),
            ),
        ],
    )
}

pub(super) fn make_file_type(path: &std::path::Path) -> Value {
    // `DirEntry::file_type` doesn't follow symlinks, like real std
    let ft = path.symlink_metadata().map(|m| m.file_type());
    let is = |f: &dyn Fn(&std::fs::FileType) -> bool| Value::Bool(ft.as_ref().is_ok_and(f));
    Value::struct_of(
        "FileType",
        [
            ("is_dir".into(), is(&|t| t.is_dir())),
            ("is_file".into(), is(&|t| t.is_file())),
            ("is_symlink".into(), is(&|t| t.is_symlink())),
        ],
    )
}

pub(super) fn path_string(s: &StructData, key: &str) -> String {
    s.get(key).map(|v| v.display()).unwrap_or_default()
}

pub(super) fn path_method(
    st: &Arc<StructData>,
    method: &MethodName,
    args: &[Value],
) -> Result<Value> {
    let s = path_string(st, "s");
    let p = std::path::Path::new(&s);
    let opt_str = |o: Option<&std::ffi::OsStr>| match o {
        Some(v) => Value::some(Value::str(v.to_string_lossy().into_owned())),
        None => Value::none(),
    };
    Ok(match method.id {
        BuiltinId::Display | BuiltinId::ToStringLossy => Value::str(s.clone()),
        BuiltinId::ToStr => Value::some(Value::str(s.clone())),
        BuiltinId::IntoString | BuiltinId::IntoOsString => Value::ok(Value::str(s.clone())),
        BuiltinId::ToOwned
        | BuiltinId::ToPathBuf
        | BuiltinId::Clone
        | BuiltinId::AsPath
        | BuiltinId::AsOsStr => make_path(s.clone()),
        BuiltinId::IsDir => Value::Bool(p.is_dir()),
        BuiltinId::IsFile => Value::Bool(p.is_file()),
        BuiltinId::IsAbsolute => Value::Bool(p.is_absolute()),
        BuiltinId::Exists => Value::Bool(p.exists()),
        BuiltinId::FileName => match p.file_name() {
            Some(n) => Value::some(make_path(n.to_string_lossy().into_owned())),
            None => Value::none(),
        },
        BuiltinId::FileStem => opt_str(p.file_stem()),
        BuiltinId::Extension => opt_str(p.extension()),
        BuiltinId::WithExtension => {
            make_path(p.with_extension(arg_str(args, 0)).display().to_string())
        }
        BuiltinId::Parent => match p.parent() {
            Some(par) => Value::some(make_path(par.display().to_string())),
            None => Value::none(),
        },
        BuiltinId::Ancestors => Value::vec(
            p.ancestors()
                .map(|ancestor| make_path(ancestor.display().to_string()))
                .collect(),
        ),
        BuiltinId::Join | BuiltinId::Push => {
            let joined = p.join(args.first().map(Value::display).unwrap_or_default());
            make_path(joined.display().to_string())
        }
        // Path compares whole components, so "/a/bc" doesn't start with "/a/b"
        BuiltinId::StartsWith => {
            Value::Bool(p.starts_with(args.first().map(Value::display).unwrap_or_default()))
        }
        BuiltinId::EndsWith => {
            Value::Bool(p.ends_with(args.first().map(Value::display).unwrap_or_default()))
        }
        _ => bail!("unknown method `{method}` on Path"),
    })
}

pub(super) fn dir_entry_method(s: &Arc<StructData>, method: &MethodName) -> Result<Value> {
    let path = path_string(s, "path");
    Ok(match method.id {
        BuiltinId::Path => make_path(path),
        BuiltinId::FileName => make_path(path_string(s, "name")),
        BuiltinId::FileType => Value::ok(make_file_type(std::path::Path::new(&path))),
        _ => bail!("unknown method `{method}` on DirEntry"),
    })
}

pub(super) fn file_type_method(s: &Arc<StructData>, method: &MethodName) -> Result<Value> {
    let get = |k: &str| s.get(k).unwrap_or(Value::Bool(false));
    Ok(match method.id {
        BuiltinId::IsDir => get("is_dir"),
        BuiltinId::IsFile => get("is_file"),
        BuiltinId::IsSymlink => get("is_symlink"),
        _ => bail!("unknown method `{method}` on FileType"),
    })
}

pub(super) fn metadata_method(s: &Arc<StructData>, name: &MethodName) -> Result<Value> {
    let get = |k: &str| {
        s.get(k)
            .ok_or_else(|| anyhow!("Metadata has no `{k}` field"))
    };
    Ok(match name.id {
        BuiltinId::Len => get("len")?,
        BuiltinId::IsDir => get("is_dir")?,
        BuiltinId::IsFile => get("is_file")?,
        BuiltinId::IsSymlink => get("is_symlink")?,
        BuiltinId::Modified | BuiltinId::Created | BuiltinId::Accessed => match s.get("modified") {
            Some(v) => Value::ok(v),
            None => Value::err(Value::str("timestamp not available".to_string())),
        },
        BuiltinId::Mode
        | BuiltinId::Dev
        | BuiltinId::Ino
        | BuiltinId::Uid
        | BuiltinId::Gid
        | BuiltinId::Mtime => get(&name.text)?,
        BuiltinId::Permissions => Value::struct_of(
            "Permissions",
            [
                ("mode".into(), get("mode")?),
                ("readonly".into(), get("readonly")?),
            ],
        ),
        _ => bail!("unknown method `{name}` on Metadata"),
    })
}

pub(super) fn wrap_io(r: std::io::Result<String>) -> Value {
    match r {
        Ok(s) => Value::ok(Value::str(s)),
        Err(e) => Value::err(super::native::io_error_value(&e)),
    }
}

pub(super) fn wrap_bytes(r: std::io::Result<Vec<u8>>) -> Value {
    match r {
        Ok(bytes) => Value::ok(Value::vec(
            bytes
                .into_iter()
                .map(|b| Value::Int(i64::from(b)))
                .collect(),
        )),
        Err(e) => Value::err(super::native::io_error_value(&e)),
    }
}

pub(super) fn wrap_unit(r: std::io::Result<()>) -> Value {
    match r {
        Ok(()) => Value::ok(Value::Unit),
        Err(e) => Value::err(super::native::io_error_value(&e)),
    }
}

pub(super) fn field_int(s: &StructData, k: &str) -> i64 {
    s.get(k).as_ref().and_then(as_i64).unwrap_or(0)
}

pub(super) fn arg_str(args: &[Value], i: usize) -> String {
    args.get(i).map(path_like).unwrap_or_default()
}

pub(super) fn arg_int(args: &[Value], i: usize) -> i64 {
    match args.get(i) {
        Some(Value::Int(n)) => *n,
        _ => 0,
    }
}

pub(super) fn open_file(path: &str, opts: &std::fs::OpenOptions) -> Value {
    match opts.open(path) {
        Ok(f) => Value::ok(Native::File(std::io::BufReader::new(f)).wrap()),
        Err(e) => Value::err(super::native::io_error_value(&e)),
    }
}

// the setters return a fresh struct with 1 flag flipped, `open` builds a real `OpenOptions` from
// the flags
pub(super) fn openoptions_method(
    s: &StructData,
    name: &MethodName,
    args: &[Value],
) -> Result<Value> {
    const FLAGS: [&str; 6] = [
        "read",
        "write",
        "append",
        "create",
        "create_new",
        "truncate",
    ];
    let field_bool = |k: &str| matches!(s.get(k), Some(Value::Bool(true)));
    if FLAGS.contains(&name.text.as_str()) {
        let on = matches!(args.first(), Some(Value::Bool(true)));
        let pairs = FLAGS.iter().map(|&k| {
            (
                Arc::from(k),
                Value::Bool(if k == name.text { on } else { field_bool(k) }),
            )
        });
        return Ok(Value::struct_of("OpenOptions", pairs));
    }
    if name.id == BuiltinId::Open {
        let path = args.first().map(path_like).unwrap_or_default();
        let mut opts = std::fs::OpenOptions::new();
        opts.read(field_bool("read"))
            .write(field_bool("write"))
            .append(field_bool("append"))
            .create(field_bool("create"))
            .create_new(field_bool("create_new"))
            .truncate(field_bool("truncate"));
        return Ok(open_file(&path, &opts));
    }
    bail!("unknown method `{name}` on OpenOptions")
}

pub(super) fn bytes_to_string(arg: Option<&Value>) -> String {
    match arg {
        Some(Value::Str(s)) => s.to_string(),
        Some(Value::Vec(v)) => {
            let bytes: Vec<u8> = v
                .lock()
                .iter()
                .filter_map(|x| as_i64(x).and_then(|i| u8::try_from(i).ok()))
                .collect();
            String::from_utf8_lossy(&bytes).into_owned()
        }
        _ => String::new(),
    }
}
