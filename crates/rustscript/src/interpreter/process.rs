//! The `Command`, `Child`, and process output bridge. Children and their
//! pipes are native handles, so a spawned process can be driven from
//! concurrent tasks.

use std::sync::Arc;

use anyhow::{Result, bail};
use parking_lot::Mutex;

use super::native::Native;
use super::native_methods;
use super::std_bridge::path_like;
use super::value::{StructData, Value};

/// Build a real `Command` from a script `Command` value's fields. Every field
/// that becomes an OS string goes through `path_like`, so a `Path` or `PathBuf`
/// value contributes its path, not its struct debug form. `current_dir` was the
/// sharp edge, a debug string there made every spawn fail with ENOENT.
pub(super) fn build_command(s: &StructData) -> std::process::Command {
    let program = s.get("program").map(|v| path_like(&v)).unwrap_or_default();
    let mut cmd = std::process::Command::new(&program);
    if let Some(Value::Vec(a)) = s.get("args") {
        for item in a.lock().iter() {
            cmd.arg(path_like(item));
        }
    }
    // Unset builder fields hold Unit placeholders, see `Command::new`.
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

/// Run a `Command` value once it has been fully built, returning an `Output`.
pub(super) fn run_command(s: &StructData) -> Value {
    // output() pipes by default but explicit stdio settings win, so an
    // interactive child can keep the terminal while stdout is captured,
    // matching the real std behavior.
    let mut cmd = build_command(s);
    cmd.stdin(stdio_or(s, "stdin", std::process::Stdio::null()));
    cmd.stdout(stdio_or(s, "stdout", std::process::Stdio::piped()));
    cmd.stderr(stdio_or(s, "stderr", std::process::Stdio::piped()));
    match cmd.output() {
        Ok(out) => Value::ok(make_output(out)),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

/// Run a `Command` value with `.status()`, which inherits the terminal by
/// default just like real Rust, and return `Ok(ExitStatus)`.
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

/// Map a stored `Stdio` marker to a real `std::process::Stdio`, defaulting to
/// inherit so a spawned child shares the terminal like a shell command.
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

/// The real `Stdio` behind an `Stdio::from(file)` marker. The handle is cloned,
/// so the script's own `File` value stays usable after the child takes its copy.
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

/// Spawn a `Command`, returning a `Child` value whose stdin/stdout/stderr
/// fields hold the piped ends as native handles.
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
            // An alias of the stdin handle under a name no script can
            // reach, `cargo check` rejects the field. `stdin.take()`
            // empties the visible field for real, and the close on wait
            // must still find the pipe, see `child_method`.
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

/// Build an `ExitStatus` value with `code` and `success`.
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

/// Build an `Output` value with `stdout`, `stderr`, and `status`.
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

pub(super) fn command_method(recv: &Value, name: &str, args: &[Value]) -> Result<Value> {
    let Value::Struct(s) = recv else {
        unreachable!()
    };
    let cmd_value = || recv.clone();
    Ok(match name {
        "arg" => {
            if let Some(Value::Vec(list)) = s.get("args") {
                list.lock()
                    .push(args.first().cloned().unwrap_or(Value::Unit));
            }
            cmd_value()
        }
        "args" => {
            if let (Some(Value::Vec(list)), Some(Value::Vec(extra))) = (s.get("args"), args.first())
            {
                let extra = extra.lock().clone();
                list.lock().extend(extra);
            }
            cmd_value()
        }
        "current_dir" => {
            s.set("cwd", args.first().cloned().unwrap_or(Value::Unit));
            cmd_value()
        }
        "env" => {
            let key = args.first().map(Value::display).unwrap_or_default();
            let val = args.get(1).cloned().unwrap_or(Value::Unit);
            let envs = command_envs(s);
            if let Some(k) = Value::str(key).as_key() {
                envs.lock().insert(k, val);
            }
            cmd_value()
        }
        "env_remove" => {
            let key = args.first().map(Value::display).unwrap_or_default();
            let envs = command_envs(s);
            if let Some(k) = Value::str(key).as_key() {
                envs.lock().insert(k, Value::Unit);
            }
            cmd_value()
        }
        // Rejected rather than stored when it is not an Stdio, because a value
        // this does not understand used to be kept and then quietly ignored
        // when the command was built.
        "stdin" | "stdout" | "stderr" => {
            let arg = args.first().cloned().unwrap_or(Value::Unit);
            match &arg {
                Value::Struct(m) if &**m.name() == "Stdio" => {}
                other => bail!(
                    "Command::{name} takes an Stdio, got {}. Use Stdio::piped(), Stdio::null(), Stdio::inherit(), or Stdio::from(file).",
                    other.type_name()
                ),
            }
            s.set(name, arg);
            cmd_value()
        }
        "spawn" => spawn_command(s),
        "output" => run_command(s),
        "status" => status_command(s),
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

/// The hidden alias of the child's stdin handle, see `spawn_command`. A
/// constant rather than a literal so the surface harvest, which reads every
/// string in `child_method`, does not list it as a callable method.
const STDIN_PIPE: &str = "stdin_pipe";

/// Drop the real `ChildStdin` inside a shared handle, closing the pipe. Walks a
/// `Some(Native)` wrapper from `child.stdin.take()`.
fn close_child_stdin(v: &Value) {
    match v {
        Value::Native(h) => *h.lock() = Native::Taken,
        Value::Enum {
            enum_name,
            variant,
            data,
        } if &**enum_name == "Option" && &**variant == "Some" => {
            if let Some(inner) = data.first() {
                close_child_stdin(inner);
            }
        }
        _ => {}
    }
}

/// Methods on a spawned `Child`. Lifecycle calls delegate to the real child
/// handle; `wait_with_output` reads any piped stdout/stderr to the end first.
pub(super) fn child_method(recv: &Value, name: &str, args: &mut [Value]) -> Result<Value> {
    let Value::Struct(s) = recv else {
        unreachable!()
    };
    // Waiting on a child that was fed piped stdin must first close that pipe,
    // or the child blocks forever on EOF. Real Rust closes it when the taken
    // `ChildStdin` drops. The VM keeps every value alive in a register for the
    // whole call, so close it through the shared handle instead. The hidden
    // `stdin_pipe` alias reaches the pipe even after `stdin.take()` emptied
    // the visible field.
    if matches!(name, "wait" | "wait_with_output") {
        if let Some(v) = s.get(STDIN_PIPE) {
            close_child_stdin(&v);
        }
        s.set("stdin", Value::none());
        s.set(STDIN_PIPE, Value::none());
    }
    if name == "wait_with_output" {
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

/// Read a child's piped stdout/stderr field to the end as a string.
fn drain_child_pipe(s: &StructData, key: &str) -> String {
    let handle = match s.get(key) {
        Some(Value::Enum { data, .. }) => match data.first() {
            Some(Value::Native(h)) => h.clone(),
            _ => return String::new(),
        },
        _ => return String::new(),
    };
    let mut target = [Value::str("")];
    match native_methods::native_method(&handle, "read_to_string", &mut target) {
        Ok(_) => {}
        Err(_) => return String::new(),
    }
    if let Value::Str(out) = &target[0] {
        out.to_string()
    } else {
        String::new()
    }
}
