//! Bridges for `std` paths a script calls: fs, io, env, paths, metadata, and
//! streams.

use std::sync::Arc;

use anyhow::{Result, bail};

use super::bytecode::{BuiltinId as B, MethodName, PathId as P};
use super::crates_bridge::crate_bridge;
use super::enum_def::{NOT_PRESENT, NOT_UNICODE, VAR_ERROR};
use super::json_bridge::bridge_serde_json;
use super::native::Native;
use super::native_methods;
use super::value::{StructData, Value};

/// The `std::fs` free functions.
fn fs_native_call(id: P, args: &[Value]) -> Result<Option<Value>> {
    let s = |i: usize| -> Result<String> {
        match args.get(i) {
            Some(v) => Ok(path_like(v)),
            None => bail!("missing argument {i} for {id}"),
        }
    };
    Ok(Some(match id {
        P::FsReadToString => wrap_io(std::fs::read_to_string(s(0)?)),
        P::FsRead => wrap_bytes(std::fs::read(s(0)?)),
        P::FsWrite => wrap_unit(std::fs::write(s(0)?, s(1)?)),
        P::FsCreateDirAll => wrap_unit(std::fs::create_dir_all(s(0)?)),
        P::FsCreateDir => wrap_unit(std::fs::create_dir(s(0)?)),
        P::FsRemoveFile => wrap_unit(std::fs::remove_file(s(0)?)),
        P::FsRemoveDirAll => wrap_unit(std::fs::remove_dir_all(s(0)?)),
        P::FsRemoveDir => wrap_unit(std::fs::remove_dir(s(0)?)),
        P::FsCopy => match std::fs::copy(s(0)?, s(1)?) {
            Ok(n) => Value::ok(Value::Int(i64::try_from(n).unwrap_or(i64::MAX))),
            Err(e) => Value::err(super::native::io_error_value(&e)),
        },
        P::FsRename => wrap_unit(std::fs::rename(s(0)?, s(1)?)),
        P::FsReadDir => match std::fs::read_dir(s(0)?) {
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
        P::FsCanonicalize => match std::fs::canonicalize(s(0)?) {
            Ok(p) => Value::ok(make_path(p.display().to_string())),
            Err(e) => Value::err(super::native::io_error_value(&e)),
        },
        P::FsMetadata => match std::fs::metadata(s(0)?) {
            Ok(m) => Value::ok(make_metadata(&m)),
            Err(e) => Value::err(super::native::io_error_value(&e)),
        },
        P::FsSymlinkMetadata => match std::fs::symlink_metadata(s(0)?) {
            Ok(m) => Value::ok(make_metadata(&m)),
            Err(e) => Value::err(super::native::io_error_value(&e)),
        },
        P::FsReadLink => match std::fs::read_link(s(0)?) {
            Ok(p) => Value::ok(make_path(p.display().to_string())),
            Err(e) => Value::err(super::native::io_error_value(&e)),
        },
        P::FsHardLink => wrap_unit(std::fs::hard_link(s(0)?, s(1)?)),
        // The platform specific names are aliased to one cross-platform
        // helper, so the cfg gated `use` a script needs to type-check on
        // each os all dispatch here at runtime.
        P::FsSymlink | P::FsSymlinkFile | P::FsSymlinkDir => {
            wrap_unit(make_symlink(&s(0)?, &s(1)?))
        }
        P::FsSetPermissions => wrap_unit(set_permissions_impl(
            &s(0)?,
            args.get(1).and_then(perm_mode),
        )),
        _ => return Ok(None),
    }))
}

pub(super) fn native_call(id: P, args: &[Value]) -> Result<Option<Value>> {
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
        P::SerdeJsonFromStr
        | P::SerdeJsonToString
        | P::SerdeJsonToStringPretty
        | P::SerdeJsonToValue => return bridge_serde_json(id, args).map(Some),
        P::EnvArgs => Value::vec(super::script_args().into_iter().map(Value::str).collect()),
        P::EnvVar => match std::env::var(s(0)?) {
            Ok(v) => Value::ok(Value::str(v)),
            // The structured `VarError`, so `Err(VarError::NotPresent)`
            // matches and `{e:?}` prints `NotPresent` like real Rust.
            Err(std::env::VarError::NotPresent) => {
                Value::err(Value::enum_of(&VAR_ERROR, NOT_PRESENT, Vec::new()))
            }
            Err(std::env::VarError::NotUnicode(os)) => Value::err(Value::enum_of(
                &VAR_ERROR,
                NOT_UNICODE,
                vec![Value::str(os.to_string_lossy().into_owned())],
            )),
        },
        P::EnvCurrentDir => match std::env::current_dir() {
            Ok(p) => Value::ok(make_path(p.display().to_string())),
            Err(e) => Value::err(super::native::io_error_value(&e)),
        },
        P::EnvSetVar => {
            // Safety: scripts treat the environment as script-wide state, the
            // same trade a single threaded interpreter always made.
            unsafe { std::env::set_var(s(0)?, s(1)?) };
            Value::Unit
        }
        P::EnvRemoveVar => {
            unsafe { std::env::remove_var(s(0)?) };
            Value::Unit
        }
        P::EnvVarOs => match std::env::var_os(s(0)?) {
            Some(v) => Value::some(make_os_string(v.to_string_lossy().into_owned())),
            None => Value::none(),
        },
        P::EnvVars | P::EnvVarsOs => Value::vec(
            std::env::vars()
                .map(|(k, v)| Value::tuple(vec![Value::str(k), Value::str(v)]))
                .collect(),
        ),
        P::EnvSetCurrentDir => wrap_unit(std::env::set_current_dir(s(0)?)),
        P::EnvTempDir => make_path(std::env::temp_dir().display().to_string()),
        P::ProcessExit => {
            let code = args
                .first()
                .and_then(as_i64)
                .and_then(|c| i32::try_from(c).ok())
                .unwrap_or(0);
            std::process::exit(code);
        }
        P::ProcessAbort => std::process::abort(),
        P::ProcessId => Value::Int(i64::from(std::process::id())),
        // -- io -------------------------------------------------------
        P::IoStdin => make_std_stream(
            "stdin",
            Native::Reader(std::io::BufReader::new(Box::new(std::io::stdin()))),
        ),
        P::IoStdout => make_std_stream("stdout", Native::Writer(Box::new(std::io::stdout()))),
        P::IoStderr => make_std_stream("stderr", Native::Writer(Box::new(std::io::stderr()))),
        _ => return crate_bridge(id, args),
    }))
}

/// A symlink helper that picks the right platform call. On Windows a file vs
/// dir symlink needs distinct functions; the target kind comes from whether
/// the source exists as a directory.
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

pub(super) fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Int(i) => Some(*i),
        _ => None,
    }
}

/// Turn a value into a path string. A `Path`/`PathBuf` value carries the path
/// in its `s` field; an `OsString` carries its text there too. Anything else
/// uses its display form.
pub(super) fn path_like(v: &Value) -> String {
    match v {
        Value::Str(s) => s.to_string(),
        Value::Struct(st) if matches!(&**st.name(), "Path" | "PathBuf" | "OsString") => {
            st.get("s").map(|s| s.display()).unwrap_or_default()
        }
        other => other.display(),
    }
}

/// Wrap a std stream handle so `is_terminal` can name its stream while reads
/// and writes delegate to the inner native handle.
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
    if name.id == B::IsTerminal {
        let kind = s.get("kind").map(|v| v.display()).unwrap_or_default();
        let tty = match kind.as_str() {
            "stdin" => std::io::stdin().is_terminal(),
            "stderr" => std::io::stderr().is_terminal(),
            _ => std::io::stdout().is_terminal(),
        };
        return Ok(Value::Bool(tty));
    }
    if matches!(name.id, B::Lock | B::ByRef) {
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

/// Turn a script `Duration` value into a real `std::time::Duration`.
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

/// Build a `Duration` value carrying whole and sub-second parts.
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

/// Build a `Metadata` value with the common accessors materialized as fields.
/// The Unix `MetadataExt` fields are gated so the interpreter still builds on
/// Windows, where a script would use different accessors.
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

// -- path, directory entry, and file type ----------------------------------

pub(super) fn make_path(s: impl Into<String>) -> Value {
    Value::struct_of("Path", [("s".into(), Value::str(s.into()))])
}

pub(super) fn make_os_string(s: impl Into<String>) -> Value {
    Value::struct_of("OsString", [("s".into(), Value::str(s.into()))])
}

pub(super) fn os_string_method(s: &Arc<StructData>, method: &MethodName) -> Result<Value> {
    let value = s.get("s").map(|value| value.display()).unwrap_or_default();
    Ok(match method.id {
        B::Into => make_path(value),
        B::ToStringLossy | B::ToStr => Value::str(value),
        B::IsEmpty => Value::Bool(value.is_empty()),
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
    // DirEntry::file_type does not follow symlinks, so a symlink to a dir
    // reports is_symlink, not is_dir, same as the real std.
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
        B::Display | B::ToStringLossy => Value::str(s.clone()),
        B::ToStr => Value::some(Value::str(s.clone())),
        B::IntoString | B::IntoOsString => Value::ok(Value::str(s.clone())),
        B::ToOwned | B::ToPathBuf | B::Clone | B::AsPath | B::AsOsStr => make_path(s.clone()),
        B::IsDir => Value::Bool(p.is_dir()),
        B::IsFile => Value::Bool(p.is_file()),
        B::IsAbsolute => Value::Bool(p.is_absolute()),
        B::Exists => Value::Bool(p.exists()),
        B::FileName => match p.file_name() {
            Some(n) => Value::some(make_path(n.to_string_lossy().into_owned())),
            None => Value::none(),
        },
        B::FileStem => opt_str(p.file_stem()),
        B::Extension => opt_str(p.extension()),
        B::WithExtension => make_path(p.with_extension(arg_str(args, 0)).display().to_string()),
        B::Parent => match p.parent() {
            Some(par) => Value::some(make_path(par.display().to_string())),
            None => Value::none(),
        },
        B::Ancestors => Value::vec(
            p.ancestors()
                .map(|ancestor| make_path(ancestor.display().to_string()))
                .collect(),
        ),
        B::Join | B::Push => {
            let joined = p.join(args.first().map(Value::display).unwrap_or_default());
            make_path(joined.display().to_string())
        }
        // Path compares whole components, so "/a/bc" does not start with "/a/b"
        // the way the str method would say it does.
        B::StartsWith => {
            Value::Bool(p.starts_with(args.first().map(Value::display).unwrap_or_default()))
        }
        B::EndsWith => {
            Value::Bool(p.ends_with(args.first().map(Value::display).unwrap_or_default()))
        }
        _ => bail!("unknown method `{method}` on Path"),
    })
}

pub(super) fn dir_entry_method(s: &Arc<StructData>, method: &MethodName) -> Result<Value> {
    let path = path_string(s, "path");
    Ok(match method.id {
        B::Path => make_path(path),
        B::FileName => make_path(path_string(s, "name")),
        B::FileType => Value::ok(make_file_type(std::path::Path::new(&path))),
        _ => bail!("unknown method `{method}` on DirEntry"),
    })
}

pub(super) fn file_type_method(s: &Arc<StructData>, method: &MethodName) -> Result<Value> {
    let get = |k: &str| s.get(k).unwrap_or(Value::Bool(false));
    Ok(match method.id {
        B::IsDir => get("is_dir"),
        B::IsFile => get("is_file"),
        B::IsSymlink => get("is_symlink"),
        _ => bail!("unknown method `{method}` on FileType"),
    })
}

pub(super) fn metadata_method(s: &Arc<StructData>, name: &MethodName) -> Result<Value> {
    let get = |k: &str| s.get(k).unwrap_or(Value::Unit);
    Ok(match name.id {
        B::Len => get("len"),
        B::IsDir => get("is_dir"),
        B::IsFile => get("is_file"),
        B::IsSymlink => get("is_symlink"),
        B::Modified | B::Created | B::Accessed => match s.get("modified") {
            Some(v) => Value::ok(v),
            None => Value::err(Value::str("timestamp not available".to_string())),
        },
        B::Mode | B::Dev | B::Ino | B::Uid | B::Gid | B::Mtime => get(&name.text),
        B::Permissions => Value::struct_of(
            "Permissions",
            [
                ("mode".into(), get("mode")),
                ("readonly".into(), get("readonly")),
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
    match s.get(k) {
        Some(Value::Int(i)) => i,
        _ => 0,
    }
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

// Methods on the OpenOptions struct built by `OpenOptions::new`. The builder
// setters return a fresh struct with one flag flipped, matching the real
// `&mut self -> &mut Self` chain, and `open` assembles a real std OpenOptions
// from the flags and opens the file.
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
    if name.id == B::Open {
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
                .filter_map(|x| match x {
                    Value::Int(i) => u8::try_from(*i).ok(),
                    _ => None,
                })
                .collect();
            String::from_utf8_lossy(&bytes).into_owned()
        }
        _ => String::new(),
    }
}
