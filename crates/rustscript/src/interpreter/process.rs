//! The `Command` and `Child` bridge.

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::sync::Arc;

use anyhow::{Result, bail};
use parking_lot::Mutex;

use super::bridge::arg;
use super::bytecode::{BuiltinId, MethodName};
use super::enum_def::{EnumKind, SOME};
use super::native::Native;
use super::native_methods;
use super::std_bridge::path_like;
use super::value::{StructData, Value};

/// Every OS string field goes through `path_like`. A `PathBuf` debug string in `current_dir`
/// makes every spawn fail with ENOENT.
pub(super) fn build_command(s: &StructData) -> std::process::Command {
    let program = s.get("program").map(|v| path_like(&v)).unwrap_or_default();
    let mut cmd = std::process::Command::new(&program);
    if let Some(Value::Vec(a)) = s.get("args") {
        for item in a.lock().iter() {
            #[cfg(windows)]
            if let Some(text) = raw_arg_text(item) {
                cmd.raw_arg(text);
                continue;
            }
            cmd.arg(path_like(item));
        }
    }
    match s.get("cwd") {
        Some(Value::Unit) | None => {}
        Some(cwd) => {
            cmd.current_dir(path_like(&cwd));
        }
    }
    if let Some(Value::Map(envs, _)) = s.get("envs") {
        for (k, v) in envs.lock().iter() {
            let key = path_like(&k.to_value());
            if matches!(v, Value::Unit) {
                cmd.env_remove(key);
            } else {
                cmd.env(key, path_like(v));
            }
        }
    }
    cmd
}

/// A raw argument sits in the same list as the escaped ones, so the order of `arg` and `raw_arg`
/// calls is kept.
const RAW_ARG: &str = "RawArg";

/// A constant and not a literal, so the surface harvest doesn't list it as a method.
const RAW_ARG_TEXT: &str = "text";

/// Windows only, `raw_arg` refuses to run anywhere else, so no raw argument reaches the list there.
#[cfg(windows)]
fn raw_arg_text(item: &Value) -> Option<String> {
    match item {
        Value::Struct(m) if &**m.name() == RAW_ARG => m.get(RAW_ARG_TEXT).map(|v| path_like(&v)),
        _ => None,
    }
}

pub(super) fn run_command(s: &StructData) -> Value {
    // `output()` pipes by default but explicit stdio settings win, like real std
    let mut cmd = build_command(s);
    cmd.stdin(stdio_or(s, "stdin", std::process::Stdio::null()));
    cmd.stdout(stdio_or(s, "stdout", std::process::Stdio::piped()));
    cmd.stderr(stdio_or(s, "stderr", std::process::Stdio::piped()));
    match cmd.output() {
        Ok(out) => Value::ok(make_output(out)),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

/// `.status()` inherits the terminal by default.
pub(super) fn status_command(s: &StructData) -> Value {
    let mut cmd = build_command(s);
    cmd.stdin(stdio_for(s, "stdin"));
    cmd.stdout(stdio_for(s, "stdout"));
    cmd.stderr(stdio_for(s, "stderr"));
    match cmd.status() {
        Ok(status) => Value::ok(make_exit_status(status)),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

/// Defaults to inherit so a child shares the terminal.
fn stdio_for(s: &StructData, key: &str) -> std::process::Stdio {
    stdio_or(s, key, std::process::Stdio::inherit())
}

fn stdio_or(s: &StructData, key: &str, default: std::process::Stdio) -> std::process::Stdio {
    match s.get(key) {
        Some(Value::Struct(m)) if &**m.name() == "Stdio" => {
            match m.get("kind").map(|v| v.display()).as_deref() {
                Some("piped") => std::process::Stdio::piped(),
                Some("null") => std::process::Stdio::null(),
                Some("file") => {
                    stdio_from_file(m.get("file")).unwrap_or_else(std::process::Stdio::null)
                }
                _ => std::process::Stdio::inherit(),
            }
        }
        _ => default,
    }
}

/// The handle is cloned, so the script's `File` stays usable.
fn stdio_from_file(file: Option<Value>) -> Option<std::process::Stdio> {
    let Some(Value::Native(handle)) = file else {
        return None;
    };
    let locked = handle.lock();
    let Native::File(reader) = &*locked else {
        return None;
    };
    reader
        .get_ref()
        .try_clone()
        .ok()
        .map(std::process::Stdio::from)
}

pub(super) fn spawn_command(s: &StructData) -> Value {
    let mut cmd = build_command(s);
    cmd.stdin(stdio_for(s, "stdin"));
    cmd.stdout(stdio_for(s, "stdout"));
    cmd.stderr(stdio_for(s, "stderr"));
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Value::err(Value::str(e.to_string())),
    };
    let stdin = child
        .stdin
        .take()
        .map(|w| Native::ChildStdin(w).wrap())
        .map_or_else(Value::none, Value::some);
    let stdout = child
        .stdout
        .take()
        .map(reader_value)
        .map_or_else(Value::none, Value::some);
    let stderr = child
        .stderr
        .take()
        .map(reader_value)
        .map_or_else(Value::none, Value::some);
    Value::ok(Value::struct_of(
        "Child",
        [
            ("handle".into(), Native::Child(child).wrap()),
            // a hidden alias of the stdin handle, so the close on wait still finds the pipe after
            // `stdin.take()`, see `child_method`
            (STDIN_PIPE.into(), stdin.clone()),
            ("stdin".into(), stdin),
            ("stdout".into(), stdout),
            ("stderr".into(), stderr),
        ],
    ))
}

fn reader_value(r: impl std::io::Read + Send + 'static) -> Value {
    Native::Reader(std::io::BufReader::new(
        Box::new(r) as Box<dyn std::io::Read + Send>
    ))
    .wrap()
}

pub(super) fn make_exit_status(status: std::process::ExitStatus) -> Value {
    Value::struct_of(
        "ExitStatus",
        [
            (
                "code".into(),
                Value::Int(i64::from(status.code().unwrap_or(-1))),
            ),
            ("success".into(), Value::Bool(status.success())),
        ],
    )
}

pub(super) fn make_output(out: std::process::Output) -> Value {
    Value::struct_of(
        "Output",
        [
            (
                "stdout".into(),
                Value::str(super::console::decode(&out.stdout)),
            ),
            (
                "stderr".into(),
                Value::str(super::console::decode(&out.stderr)),
            ),
            ("status".into(), make_exit_status(out.status)),
        ],
    )
}

pub(super) fn command_method(recv: &Value, name: &MethodName, args: &[Value]) -> Result<Value> {
    let Value::Struct(s) = recv else {
        unreachable!()
    };
    let cmd_value = || recv.clone();
    Ok(match name.id {
        BuiltinId::Arg => {
            if let Some(Value::Vec(list)) = s.get("args") {
                list.lock().push(arg(args, 0)?);
            }
            cmd_value()
        }
        // `std::os::windows::process::CommandExt`, the text reaches the child with no escaping
        BuiltinId::RawArg => {
            if !cfg!(windows) {
                bail!(
                    "Command::raw_arg exists on Windows only, put the call behind #[cfg(windows)]"
                );
            }
            if let Some(Value::Vec(list)) = s.get("args") {
                list.lock().push(Value::struct_of(
                    RAW_ARG,
                    [(RAW_ARG_TEXT.into(), arg(args, 0)?)],
                ));
            }
            cmd_value()
        }
        BuiltinId::Args => {
            if let (Some(Value::Vec(list)), Some(Value::Vec(extra))) = (s.get("args"), args.first())
            {
                let extra = extra.lock().clone();
                list.lock().extend(extra);
            }
            cmd_value()
        }
        BuiltinId::CurrentDir => {
            s.set("cwd", arg(args, 0)?);
            cmd_value()
        }
        BuiltinId::Env => {
            let key = args.first().map(Value::display).unwrap_or_default();
            let val = arg(args, 1)?;
            let envs = command_envs(s);
            if let Some(k) = Value::str(key).as_key() {
                envs.lock().insert(k, val);
            }
            cmd_value()
        }
        BuiltinId::EnvRemove => {
            let key = args.first().map(Value::display).unwrap_or_default();
            let envs = command_envs(s);
            if let Some(k) = Value::str(key).as_key() {
                envs.lock().insert(k, Value::Unit);
            }
            cmd_value()
        }
        BuiltinId::Envs => {
            let envs = command_envs(s);
            for (key, val) in env_pairs(&arg(args, 0)?)? {
                if let Some(k) = Value::str(key).as_key() {
                    envs.lock().insert(k, val);
                }
            }
            cmd_value()
        }
        // a non Stdio value must be an error, not silently ignored
        BuiltinId::Stdin | BuiltinId::Stdout | BuiltinId::Stderr => {
            let target = arg(args, 0)?;
            match &target {
                Value::Struct(m) if &**m.name() == "Stdio" => {}
                other => bail!(
                    "Command::{name} takes an Stdio, got {}. Use Stdio::piped(), Stdio::null(), Stdio::inherit(), or Stdio::from(file).",
                    other.type_name()
                ),
            }
            s.set(&name.text, target);
            cmd_value()
        }
        BuiltinId::Spawn => spawn_command(s),
        BuiltinId::Output => run_command(s),
        BuiltinId::Status => status_command(s),
        _ => bail!("unknown method `{name}` on Command"),
    })
}

fn command_envs(s: &StructData) -> super::value::Map {
    if let Some(Value::Map(envs, _)) = s.get("envs") {
        envs
    } else {
        let envs: super::value::Map = Arc::new(Mutex::new(indexmap::IndexMap::default()));
        s.set("envs", Value::Map(envs.clone(), super::value::MapKind::Map));
        envs
    }
}

/// A `HashMap` arrives as a map, an array or a drained iterator of pairs as a vec of tuples.
fn env_pairs(pairs: &Value) -> Result<Vec<(String, Value)>> {
    match pairs {
        Value::Ref(reference) => match reference.get() {
            Some(inner) => env_pairs(&inner),
            None => bail!("Command::envs got a dangling reference"),
        },
        Value::Map(map, _) => Ok(map
            .lock()
            .iter()
            .map(|(k, v)| (k.to_value().display(), v.clone()))
            .collect()),
        Value::Vec(list) => list
            .lock()
            .iter()
            .map(|pair| match pair {
                Value::Tuple(kv) if kv.lock().len() == 2 => {
                    let kv = kv.lock();
                    Ok((kv[0].display(), kv[1].clone()))
                }
                other => bail!(
                    "Command::envs needs (key, value) pairs, got {}",
                    other.type_name()
                ),
            })
            .collect(),
        other => bail!(
            "Command::envs needs an iterable of pairs, got {}",
            other.type_name()
        ),
    }
}

/// A constant and not a literal, so the surface harvest doesn't list it as a method.
const STDIN_PIPE: &str = "stdin_pipe";

/// Walks a `Some(Native)` wrapper from `child.stdin.take()`.
fn close_child_stdin(v: &Value) {
    match v {
        Value::Native(h) => *h.lock() = Native::Taken,
        Value::Enum { def, variant, data } if def.kind == EnumKind::Option && *variant == SOME => {
            let first = data.lock().first().cloned();
            if let Some(inner) = first {
                close_child_stdin(&inner);
            }
        }
        _ => {}
    }
}

pub(super) fn child_method(recv: &Value, name: &MethodName, args: &mut [Value]) -> Result<Value> {
    let Value::Struct(s) = recv else {
        unreachable!()
    };
    // The stdin pipe must close before waiting or the child blocks on EOF. The VM keeps values alive
    // in registers, so close it through the hidden alias.
    if matches!(name.id, BuiltinId::Wait | BuiltinId::WaitWithOutput) {
        if let Some(v) = s.get(STDIN_PIPE) {
            close_child_stdin(&v);
        }
        s.set("stdin", Value::none());
        s.set(STDIN_PIPE, Value::none());
    }
    if name.id == BuiltinId::WaitWithOutput {
        let out = drain_child_pipe(s, "stdout");
        let err = drain_child_pipe(s, "stderr");
        let status = {
            let handle = child_handle(s)?;
            let mut h = handle.lock();
            if let Native::Child(c) = &mut *h {
                match c.wait() {
                    Ok(st) => st,
                    Err(e) => return Ok(Value::err(Value::str(e.to_string()))),
                }
            } else {
                bail!("child handle missing");
            }
        };
        return Ok(Value::ok(Value::struct_of(
            "Output",
            [
                ("stdout".into(), Value::str(out)),
                ("stderr".into(), Value::str(err)),
                ("status".into(), make_exit_status(status)),
            ],
        )));
    }
    let handle = child_handle(s)?;
    match native_methods::native_method(&handle, name, args)? {
        Some(v) => Ok(v),
        None => bail!("unknown method `{name}` on Child"),
    }
}

fn child_handle(s: &StructData) -> Result<Arc<Mutex<Native>>> {
    match s.get("handle") {
        Some(Value::Native(h)) => Ok(h),
        _ => bail!("child handle missing"),
    }
}

fn drain_child_pipe(s: &StructData, key: &str) -> String {
    let handle = match s.get(key) {
        Some(Value::Enum { data, .. }) => match data.lock().first().cloned() {
            Some(Value::Native(h)) => h,
            _ => return String::new(),
        },
        _ => return String::new(),
    };
    let mut target = [Value::str("")];
    match native_methods::native_method(
        &handle,
        &MethodName::builtin(BuiltinId::ReadToString),
        &mut target,
    ) {
        Ok(_) => {}
        Err(_) => return String::new(),
    }
    if let Value::Str(out) = &target[0] {
        out.to_string()
    } else {
        String::new()
    }
}
