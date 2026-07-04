//! Bridges for `std` paths a script calls: fs, io, env, paths,
//! metadata, streams. Split from `builtins.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Result, anyhow, bail};

use super::native::{self, Native};

use super::value::{Map, StructData, Value};

use super::crates_bridge::*;
use super::http::*;
use super::json_bridge::*;
use super::jwt_bridge::*;
use super::regex_bridge::*;


// -- std bridges -----------------------------------------------------------

/// Native implementations of the supported subset of std and serde_json,
/// dispatched by the last two path segments as `module::func`. Returns None
/// when the namespace is not native, so callers can try other handlers.
pub(super) fn native_call(module: &str, func: &str, args: &[Value]) -> Result<Option<Value>> {
    if module == "serde_json" {
        return bridge_serde_json(func, args).map(Some);
    }
    if module == "ureq" {
        return Ok(make_request(func, args));
    }
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
            ("fs", "create_dir") => wrap_unit(std::fs::create_dir(s(0)?)),
            ("fs", "remove_file") => wrap_unit(std::fs::remove_file(s(0)?)),
            ("fs", "remove_dir_all") => wrap_unit(std::fs::remove_dir_all(s(0)?)),
            ("fs", "remove_dir") => wrap_unit(std::fs::remove_dir(s(0)?)),
            ("fs", "copy") => match std::fs::copy(s(0)?, s(1)?) {
                Ok(n) => Value::ok(Value::Int(n as i64)),
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            ("fs", "rename") => wrap_unit(std::fs::rename(s(0)?, s(1)?)),
            ("fs", "read_dir") => match std::fs::read_dir(s(0)?) {
                Ok(rd) => {
                    let mut items = Vec::new();
                    for e in rd {
                        match e {
                            Ok(entry) => items.push(Value::ok(make_dir_entry(&entry))),
                            Err(err) => items.push(Value::err(Value::str(err.to_string()))),
                        }
                    }
                    Value::ok(Value::vec(items))
                }
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            ("fs", "canonicalize") => match std::fs::canonicalize(s(0)?) {
                Ok(p) => Value::ok(make_path(p.display().to_string())),
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            ("env", "args") => {
                Value::vec(super::script_args().into_iter().map(Value::str).collect())
            }
            ("env", "var") => match std::env::var(s(0)?) {
                Ok(v) => Value::ok(Value::str(v)),
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            ("env", "current_dir") => match std::env::current_dir() {
                Ok(p) => Value::ok(make_path(p.display().to_string())),
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            ("env", "set_var") => {
                // Safety: single threaded interpreter.
                unsafe { std::env::set_var(s(0)?, s(1)?) };
                Value::Unit
            }
            ("env", "remove_var") => {
                unsafe { std::env::remove_var(s(0)?) };
                Value::Unit
            }
            ("env", "var_os") => match std::env::var_os(s(0)?) {
                Some(v) => Value::some(Value::str(v.to_string_lossy().into_owned())),
                None => Value::none(),
            },
            ("env", "vars") | ("env", "vars_os") => Value::vec(
                std::env::vars()
                    .map(|(k, v)| Value::Tuple(Rc::new(RefCell::new(vec![Value::str(k), Value::str(v)]))))
                    .collect(),
            ),
            ("env", "set_current_dir") => wrap_unit(std::env::set_current_dir(s(0)?)),
            ("env", "temp_dir") => make_path(std::env::temp_dir().display().to_string()),
            ("process", "exit") => {
                let code = args.first().and_then(as_i64).unwrap_or(0) as i32;
                std::process::exit(code);
            }
            ("process", "abort") => std::process::abort(),
            // -- io -------------------------------------------------------
            ("io", "stdin") => make_std_stream(
                "stdin",
                Native::Reader(std::io::BufReader::new(Box::new(std::io::stdin()))),
            ),
            ("io", "stdout") => {
                make_std_stream("stdout", Native::Writer(Box::new(std::io::stdout())))
            }
            ("io", "stderr") => {
                make_std_stream("stderr", Native::Writer(Box::new(std::io::stderr())))
            }
            // -- fs metadata & links -------------------------------------
            ("fs", "metadata") => match std::fs::metadata(s(0)?) {
                Ok(m) => Value::ok(make_metadata(&m)),
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            ("fs", "symlink_metadata") => match std::fs::symlink_metadata(s(0)?) {
                Ok(m) => Value::ok(make_metadata(&m)),
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            ("fs", "read_link") => match std::fs::read_link(s(0)?) {
                Ok(p) => Value::ok(make_path(p.display().to_string())),
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            ("fs", "hard_link") => wrap_unit(std::fs::hard_link(s(0)?, s(1)?)),
            // The platform specific names are aliased to one cross-platform
            // helper, so the cfg gated `use` a script needs to type-check on
            // each os all dispatch here at runtime.
            ("fs", "symlink") | ("fs", "symlink_file") | ("fs", "symlink_dir") => {
                wrap_unit(make_symlink(&s(0)?, &s(1)?))
            }
            ("fs", "set_permissions") => {
                wrap_unit(set_permissions_impl(&s(0)?, args.get(1).and_then(perm_mode)))
            }
            // -- thread ---------------------------------------------------
            ("thread", "sleep") => {
                if let Some(d) = args.first().and_then(duration_from_value) {
                    std::thread::sleep(d);
                }
                Value::Unit
            }
            _ => return crate_bridge(module, func, args),
        }))
    }

/// The interpreter has no real threads, so a symlink helper picks the right
/// platform call. On Windows a file vs dir symlink needs distinct functions;
/// we treat the target kind by whether the source exists as a directory.
pub(super) fn make_symlink(src: &str, dst: &str) -> std::io::Result<()> {
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
        return st.get("mode").and_then(|m| as_i64(&m)).map(|m| m as u32);
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
        // Windows carries no unix mode bits, so a mode set is a no-op there.
        let _ = (path, mode);
        Ok(())
    }
}

pub(super) fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Int(i) => Some(*i),
        _ => None,
    }
}

/// Turn a value into a path string. A `Path`/`PathBuf` value carries the path in
/// its `s` field; anything else uses its display form.
pub(super) fn path_like(v: &Value) -> String {
    match v {
        Value::Str(s) => s.to_string(),
        Value::Struct(st) if &**st.name() == "Path" || &**st.name() == "PathBuf" => {
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
        [("kind".into(), Value::str(kind)), ("inner".into(), inner.wrap())],
    )
}

pub(super) fn std_stream_method(s: &Rc<StructData>, name: &str, args: &mut [Value]) -> Result<Value> {
    use std::io::IsTerminal;
    if name == "is_terminal" {
        let kind = s.get("kind").map(|v| v.display()).unwrap_or_default();
        let tty = match kind.as_str() {
            "stdin" => std::io::stdin().is_terminal(),
            "stderr" => std::io::stderr().is_terminal(),
            _ => std::io::stdout().is_terminal(),
        };
        return Ok(Value::Bool(tty));
    }
    if matches!(name, "lock" | "by_ref") {
        return Ok(Value::Struct(s.clone()));
    }
    let inner = match s.get("inner") {
        Some(Value::Native(h)) => h.clone(),
        _ => bail!("std stream lost its handle"),
    };
    match native::native_method(&inner, name, args)? {
        Some(v) => Ok(v),
        None => bail!("unknown method `{name}` on a std stream"),
    }
}

/// Turn a script `Duration` value into a real `std::time::Duration`.
pub(super) fn duration_from_value(v: &Value) -> Option<std::time::Duration> {
    if let Value::Struct(s) = v
        && &**s.name() == "Duration"
    {
        let secs = field_int(s, "secs") as u64;
        let nanos = field_int(s, "nanos") as u32;
        return Some(std::time::Duration::new(secs, nanos));
    }
    None
}

/// Build a `Duration` value carrying whole and sub-second parts.
pub(super) fn make_duration(d: std::time::Duration) -> Value {
    Value::struct_of(
        "Duration",
        [
            ("secs".into(), Value::Int(d.as_secs() as i64)),
            ("nanos".into(), Value::Int(d.subsec_nanos() as i64)),
        ],
    )
}

/// Build a `Metadata` value with the common accessors materialized as fields.
/// The Unix `MetadataExt` fields are gated so the interpreter still builds on
/// Windows, where a script would use different accessors.
pub(super) fn make_metadata(m: &std::fs::Metadata) -> Value {
    let mut f: Vec<(Rc<str>, Value)> = vec![
        ("len".into(), Value::Int(m.len() as i64)),
        ("is_dir".into(), Value::Bool(m.is_dir())),
        ("is_file".into(), Value::Bool(m.is_file())),
        ("is_symlink".into(), Value::Bool(m.is_symlink())),
        ("readonly".into(), Value::Bool(m.permissions().readonly())),
    ];
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;
        f.push(("mode".into(), Value::Int(m.permissions().mode() as i64)));
        f.push(("dev".into(), Value::Int(m.dev() as i64)));
        f.push(("ino".into(), Value::Int(m.ino() as i64)));
        f.push(("uid".into(), Value::Int(m.uid() as i64)));
        f.push(("gid".into(), Value::Int(m.gid() as i64)));
        f.push(("mtime".into(), Value::Int(m.mtime() as i64)));
    }
    if let Ok(t) = m.modified() {
        f.push(("modified".into(), super::native::Native::SystemTime(t).wrap()));
    }
    Value::struct_of("Metadata", f)
}
// -- path, directory entry, and file type ----------------------------------

pub(super) fn make_path(s: impl Into<String>) -> Value {
    Value::struct_of("Path", [("s".into(), Value::str(s.into()))])
}

pub(super) fn make_dir_entry(entry: &std::fs::DirEntry) -> Value {
    Value::struct_of(
        "DirEntry",
        [
            ("path".into(), Value::str(entry.path().display().to_string())),
            ("name".into(), Value::str(entry.file_name().to_string_lossy().into_owned())),
        ],
    )
}

pub(super) fn make_file_type(path: &std::path::Path) -> Value {
    // DirEntry::file_type does not follow symlinks, so a symlink to a dir
    // reports is_symlink, not is_dir, same as the real std.
    let ft = path.symlink_metadata().map(|m| m.file_type());
    let is = |f: &dyn Fn(&std::fs::FileType) -> bool| {
        Value::Bool(ft.as_ref().map(f).unwrap_or(false))
    };
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
    st: &StructData,
    method: &str,
    args: &[Value],
) -> Result<Value> {
    let s = path_string(st, "s");
    let p = std::path::Path::new(&s);
    let opt_str = |o: Option<&std::ffi::OsStr>| match o {
        Some(v) => Value::some(Value::str(v.to_string_lossy().into_owned())),
        None => Value::none(),
    };
    Ok(match method {
        "display" | "to_string_lossy" => Value::str(s.clone()),
        "to_str" => Value::some(Value::str(s.clone())),
        "into_string" | "into_os_string" => Value::ok(Value::str(s.clone())),
        "to_owned" | "to_path_buf" | "clone" | "as_path" | "as_os_str" => make_path(s.clone()),
        "is_dir" => Value::Bool(p.is_dir()),
        "is_file" => Value::Bool(p.is_file()),
        "is_absolute" => Value::Bool(p.is_absolute()),
        "exists" => Value::Bool(p.exists()),
        "file_name" => match p.file_name() {
            Some(n) => Value::some(make_path(n.to_string_lossy().into_owned())),
            None => Value::none(),
        },
        "file_stem" => opt_str(p.file_stem()),
        "extension" => opt_str(p.extension()),
        "parent" => match p.parent() {
            Some(par) => Value::some(make_path(par.display().to_string())),
            None => Value::none(),
        },
        "join" | "push" => {
            let joined = p.join(args.first().map(|v| v.display()).unwrap_or_default());
            make_path(joined.display().to_string())
        }
        _ => bail!("unknown method `{method}` on Path"),
    })
}

pub(super) fn dir_entry_method(
    s: &StructData,
    method: &str,
) -> Result<Value> {
    let path = path_string(s, "path");
    Ok(match method {
        "path" => make_path(path),
        "file_name" => make_path(path_string(s, "name")),
        "file_type" => Value::ok(make_file_type(std::path::Path::new(&path))),
        _ => bail!("unknown method `{method}` on DirEntry"),
    })
}

pub(super) fn file_type_method(s: &StructData, method: &str) -> Result<Value> {
    let get = |k: &str| s.get(k).unwrap_or(Value::Bool(false));
    Ok(match method {
        "is_dir" => get("is_dir"),
        "is_file" => get("is_file"),
        "is_symlink" => get("is_symlink"),
        _ => bail!("unknown method `{method}` on FileType"),
    })
}

pub(super) fn wrap_io(r: std::io::Result<String>) -> Value {
    match r {
        Ok(s) => Value::ok(Value::str(s)),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

pub(super) fn wrap_bytes(r: std::io::Result<Vec<u8>>) -> Value {
    match r {
        Ok(bytes) => Value::ok(Value::vec(bytes.into_iter().map(|b| Value::Int(b as i64)).collect())),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

pub(super) fn wrap_unit(r: std::io::Result<()>) -> Value {
    match r {
        Ok(()) => Value::ok(Value::Unit),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

pub(super) fn one(args: Vec<Value>) -> Result<Value> {
    args.into_iter()
        .next()
        .ok_or_else(|| anyhow!("expected one argument"))
}

/// Associated functions like `String::new`, `Vec::new`, `HashMap::new`.
pub(super) fn assoc_fn(ty: &str, func: &str, args: &[Value]) -> Result<Option<Value>> {
    if matches!(ty, "Header" | "EncodingKey") {
        return jwt_assoc(ty, func, args);
    }
    Ok(Some(match (ty, func) {
        ("Permissions", "from_mode") => {
            let mode = args.first().and_then(as_i64).unwrap_or(0o644);
            Value::struct_of("Permissions", vec![("mode".into(), Value::Int(mode))])
        }
        ("String", "new") | ("String", "with_capacity") => Value::str(""),
        ("String", "from") => Value::str(args.first().map(|v| v.display()).unwrap_or_default()),
        ("String", "from_utf8_lossy") => Value::str(bytes_to_string(args.first())),
        ("char", "from_u32") => match args.first().and_then(as_i64) {
            Some(n) if (0..=0x10FFFF).contains(&n) => match char::from_u32(n as u32) {
                Some(c) => Value::some(Value::Char(c)),
                None => Value::none(),
            },
            _ => Value::none(),
        },
        ("char", "from_digit") => {
            let n = args.first().and_then(as_i64).unwrap_or(-1);
            let radix = args.get(1).and_then(as_i64).unwrap_or(10);
            match (u32::try_from(n), u32::try_from(radix)) {
                (Ok(n), Ok(radix)) => match char::from_digit(n, radix) {
                    Some(c) => Value::some(Value::Char(c)),
                    None => Value::none(),
                },
                _ => Value::none(),
            }
        }
        // Every integer type parses the same way here, values are untyped ints.
        (
            "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64"
            | "u128" | "usize",
            "from_str_radix",
        ) => {
            let text = args.first().map(|v| v.display()).unwrap_or_default();
            let radix = args.get(1).and_then(as_i64).unwrap_or(10) as u32;
            match i64::from_str_radix(text.trim(), radix) {
                Ok(n) => Value::ok(Value::Int(n)),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        ("String", "from_utf8") => Value::ok(Value::str(bytes_to_string(args.first()))),
        // The shape carries every field a later builder call can set, since a
        // shape cannot grow after the instance exists.
        ("Command", "new") => Value::struct_of(
            "Command",
            [
                ("program".into(), args.first().cloned().unwrap_or_else(|| Value::str(""))),
                ("args".into(), Value::vec(vec![])),
                ("cwd".into(), Value::Unit),
                ("envs".into(), Value::Unit),
                ("stdin".into(), Value::Unit),
                ("stdout".into(), Value::Unit),
                ("stderr".into(), Value::Unit),
            ],
        ),
        ("Vec", "new") | ("Vec", "with_capacity") => Value::vec(vec![]),
        ("Vec", "from") => match args.first() {
            Some(Value::Vec(v)) => Value::vec(v.borrow().clone()),
            Some(other) => Value::vec(vec![other.clone()]),
            None => Value::vec(vec![]),
        },
        ("HashMap", "new") | ("BTreeMap", "new") | ("HashMap", "with_capacity")
        | ("HashSet", "new") | ("BTreeSet", "new") => {
            Value::Map(Rc::new(RefCell::new(Map::default())))
        }
        ("Box" | "Rc" | "Arc" | "RefCell" | "Cell", "new") => {
            args.first().cloned().unwrap_or(Value::Unit)
        }
        // Our file and pipe readers are already buffered, so wrapping is a
        // pass-through; a raw socket is turned into a buffered reader.
        ("BufReader" | "BufWriter", "new" | "with_capacity") => {
            match args.last() {
                Some(Value::Native(h)) if matches!(&*h.borrow(), Native::Stream(_)) => {
                    let Native::Stream(s) = &*h.borrow() else {
                        unreachable!()
                    };
                    match s.try_clone() {
                        Ok(clone) => Native::Reader(std::io::BufReader::new(
                            Box::new(clone) as Box<dyn std::io::Read>,
                        ))
                        .wrap(),
                        Err(e) => return Err(anyhow!("cannot buffer socket: {e}")),
                    }
                }
                other => other.cloned().unwrap_or(Value::Unit),
            }
        }
        ("PathBuf", "new") => make_path(""),
        ("PathBuf" | "Path", "from") => {
            make_path(args.first().map(|v| v.display()).unwrap_or_default())
        }
        ("Path", "new") => make_path(args.first().map(|v| v.display()).unwrap_or_default()),
        ("Regex", "new") => {
            let pat = args.first().map(|v| v.display()).unwrap_or_default();
            match regex::Regex::new(&pat) {
                Ok(_) => Value::ok(make_regex(pat)),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        ("Some", _) => Value::some(args.first().cloned().unwrap_or(Value::Unit)),
        ("Option", "Some") => Value::some(args.first().cloned().unwrap_or(Value::Unit)),
        ("Result", "Ok") => Value::ok(args.first().cloned().unwrap_or(Value::Unit)),
        ("Result", "Err") => Value::err(args.first().cloned().unwrap_or(Value::Unit)),
        // -- files -----------------------------------------------------
        ("File", "open") => open_file(&arg_str(args, 0), std::fs::OpenOptions::new().read(true)),
        ("File", "create") => open_file(
            &arg_str(args, 0),
            std::fs::OpenOptions::new().write(true).create(true).truncate(true),
        ),
        ("File", "create_new") => open_file(
            &arg_str(args, 0),
            std::fs::OpenOptions::new().write(true).create_new(true),
        ),
        ("OpenOptions", "new") => Value::struct_of(
            "OpenOptions",
            ["read", "write", "append", "create", "create_new", "truncate"]
                .into_iter()
                .map(|k| (k.into(), Value::Bool(false))),
        ),
        // -- time ------------------------------------------------------
        ("Instant", "now") => Native::Instant(std::time::Instant::now()).wrap(),
        ("SystemTime", "now") => Native::SystemTime(std::time::SystemTime::now()).wrap(),
        ("Duration", "from_secs") => make_duration(std::time::Duration::from_secs(
            arg_int(args, 0) as u64,
        )),
        ("Duration", "from_millis") => make_duration(std::time::Duration::from_millis(
            arg_int(args, 0) as u64,
        )),
        ("Duration", "from_micros") => make_duration(std::time::Duration::from_micros(
            arg_int(args, 0) as u64,
        )),
        ("Duration", "from_nanos") => make_duration(std::time::Duration::from_nanos(
            arg_int(args, 0) as u64,
        )),
        ("Duration", "new") => make_duration(std::time::Duration::new(
            arg_int(args, 0) as u64,
            arg_int(args, 1) as u32,
        )),
        // -- net -------------------------------------------------------
        ("TcpListener", "bind") => match std::net::TcpListener::bind(arg_str(args, 0)) {
            Ok(l) => Value::ok(Native::Listener(l).wrap()),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        ("TcpStream", "connect") => match std::net::TcpStream::connect(arg_str(args, 0)) {
            Ok(s) => Value::ok(Native::Stream(s).wrap()),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        ("SeekFrom", "Start" | "End" | "Current") => Value::Enum {
            enum_name: "SeekFrom".into(),
            variant: func.into(),
            data: Value::one_data(args.first().cloned().unwrap_or(Value::Int(0))),
        },
        ("Agent", "new_with_defaults") => Native::Agent(ureq::agent()).wrap(),
        ("Stdio", "piped") | ("Stdio", "inherit") | ("Stdio", "null") => {
            Value::struct_of("Stdio", [("kind".into(), Value::str(func))])
        }
        _ => return Ok(None),
    }))
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
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

/// Bridges for the extra crates a script may `use`. Reached when a
pub(super) fn opt_path(p: Option<std::path::PathBuf>) -> Value {
    match p {
        Some(p) => Value::some(make_path(p.display().to_string())),
        None => Value::none(),
    }
}

pub(super) fn bytes_arg(v: Option<&Value>) -> Vec<u8> {
    match v {
        Some(Value::Str(s)) => s.as_bytes().to_vec(),
        Some(Value::Vec(items)) => items
            .borrow()
            .iter()
            .filter_map(|x| match x {
                Value::Int(i) => Some(*i as u8),
                _ => None,
            })
            .collect(),
        Some(other) => other.display().into_bytes(),
        None => Vec::new(),
    }
}

pub(super) fn bytes_to_vec(b: &[u8]) -> Value {
    Value::vec(b.iter().map(|x| Value::Int(*x as i64)).collect())
}
pub(super) fn duration_method(s: &StructData, name: &str) -> Result<Value> {
    let secs = field_int(s, "secs") as u64;
    let nanos = field_int(s, "nanos") as u32;
    let total_nanos = secs as u128 * 1_000_000_000 + nanos as u128;
    Ok(match name {
        "as_secs" => Value::Int(secs as i64),
        "as_millis" => Value::Int((total_nanos / 1_000_000) as i64),
        "as_micros" => Value::Int((total_nanos / 1_000) as i64),
        "as_nanos" => Value::Int(total_nanos as i64),
        "subsec_nanos" => Value::Int(nanos as i64),
        "subsec_millis" => Value::Int((nanos / 1_000_000) as i64),
        "subsec_micros" => Value::Int((nanos / 1_000) as i64),
        "as_secs_f64" => Value::Float(secs as f64 + nanos as f64 / 1e9),
        "is_zero" => Value::Bool(total_nanos == 0),
        _ => bail!("unknown method `{name}` on Duration"),
    })
}

pub(super) fn metadata_method(s: &StructData, name: &str, _args: &[Value]) -> Result<Value> {
    let get = |k: &str| s.get(k).unwrap_or(Value::Unit);
    Ok(match name {
        "len" => get("len"),
        "is_dir" => get("is_dir"),
        "is_file" => get("is_file"),
        "is_symlink" => get("is_symlink"),
        "modified" | "created" | "accessed" => match s.get("modified") {
            Some(v) => Value::ok(v),
            None => Value::err(Value::str("timestamp not available".to_string())),
        },
        "mode" | "dev" | "ino" | "uid" | "gid" | "mtime" => get(name),
        "permissions" => Value::struct_of(
            "Permissions",
            [("mode".into(), get("mode")), ("readonly".into(), get("readonly"))],
        ),
        _ => bail!("unknown method `{name}` on Metadata"),
    })
}

pub(super) fn field_int(s: &StructData, k: &str) -> i64 {
    match s.get(k) {
        Some(Value::Int(i)) => i,
        _ => 0,
    }
}

pub(super) fn bytes_to_string(arg: Option<&Value>) -> String {
    match arg {
        Some(Value::Str(s)) => s.to_string(),
        Some(Value::Vec(v)) => {
            let bytes: Vec<u8> = v
                .borrow()
                .iter()
                .filter_map(|x| match x {
                    Value::Int(i) => Some(*i as u8),
                    _ => None,
                })
                .collect();
            String::from_utf8_lossy(&bytes).into_owned()
        }
        _ => String::new(),
    }
}
