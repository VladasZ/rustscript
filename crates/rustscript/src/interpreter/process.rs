//! The `Command`, `Child`, and process output bridge.
//! Split from `builtins.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Result, bail};

use super::native::{self, Native};
use super::std_bridge::path_like;
use super::value::{Map, MapKey, RStr, StructData, Value};



/// Build a real `Command` from a script `Command` value's fields. Every field
/// that becomes an OS string goes through `path_like`, so a `Path` or `PathBuf`
/// value contributes its path, not its struct debug form. current_dir was the
/// sharp edge, a debug string there made every spawn fail with ENOENT.
pub(super) fn build_command(s: &StructData) -> std::process::Command {
    let program = s.get("program").map(|v| path_like(&v)).unwrap_or_default();
    let mut cmd = std::process::Command::new(&program);
    if let Some(Value::Vec(a)) = s.get("args") {
        for item in a.borrow().iter() {
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
    if let Some(Value::Map(envs)) = s.get("envs") {
        for (k, v) in envs.borrow().iter() {
            cmd.env(path_like(&k.to_value()), path_like(v));
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
pub(super) fn stdio_for(s: &StructData, key: &str) -> std::process::Stdio {
    stdio_or(s, key, std::process::Stdio::inherit())
}

fn stdio_or(s: &StructData, key: &str, default: std::process::Stdio) -> std::process::Stdio {
    match s.get(key) {
        Some(Value::Struct(m)) if &**m.name() == "Stdio" => {
            match m.get("kind").map(|v| v.display()).as_deref() {
                Some("piped") => std::process::Stdio::piped(),
                Some("null") => std::process::Stdio::null(),
                _ => std::process::Stdio::inherit(),
            }
        }
        _ => default,
    }
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
        .map(Value::some)
        .unwrap_or_else(Value::none);
    let stdout = child
        .stdout
        .take()
        .map(|r| Native::Reader(std::io::BufReader::new(Box::new(r) as Box<dyn std::io::Read>)).wrap())
        .map(Value::some)
        .unwrap_or_else(Value::none);
    let stderr = child
        .stderr
        .take()
        .map(|r| Native::Reader(std::io::BufReader::new(Box::new(r) as Box<dyn std::io::Read>)).wrap())
        .map(Value::some)
        .unwrap_or_else(Value::none);
    Value::ok(Value::struct_of(
        "Child",
        [
            ("handle".into(), Native::Child(child).wrap()),
            ("stdin".into(), stdin),
            ("stdout".into(), stdout),
            ("stderr".into(), stderr),
        ],
    ))
}

/// Build an `ExitStatus` value with `code` and `success`.
pub(super) fn make_exit_status(status: std::process::ExitStatus) -> Value {
    Value::struct_of(
        "ExitStatus",
        [
            ("code".into(), Value::Int(status.code().unwrap_or(-1) as i64)),
            ("success".into(), Value::Bool(status.success())),
        ],
    )
}

/// Build an `Output` value with `stdout`, `stderr`, and `status`.
pub(super) fn make_output(out: std::process::Output) -> Value {
    Value::struct_of(
        "Output",
        [
            ("stdout".into(), Value::str(String::from_utf8_lossy(&out.stdout).into_owned())),
            ("stderr".into(), Value::str(String::from_utf8_lossy(&out.stderr).into_owned())),
            ("status".into(), make_exit_status(out.status)),
        ],
    )
}

pub(super) fn command_method(
    s: &Rc<StructData>,
    name: &str,
    args: &[Value],
) -> Result<Value> {
    let cmd_value = || Value::Struct(s.clone());
    Ok(match name {
        "arg" => {
            if let Some(Value::Vec(list)) = s.get("args") {
                list.borrow_mut()
                    .push(args.first().cloned().unwrap_or(Value::Unit));
            }
            cmd_value()
        }
        "args" => {
            if let (Some(Value::Vec(list)), Some(Value::Vec(extra))) =
                (s.get("args"), args.first())
            {
                list.borrow_mut().extend(extra.borrow().iter().cloned());
            }
            cmd_value()
        }
        "current_dir" => {
            s.set("cwd", args.first().cloned().unwrap_or(Value::Unit));
            cmd_value()
        }
        "env" => {
            let key = args.first().map(|v| v.display()).unwrap_or_default();
            let val = args.get(1).cloned().unwrap_or(Value::Unit);
            let envs = match s.get("envs") {
                Some(Value::Map(m)) => m,
                _ => {
                    let m = Rc::new(RefCell::new(Map::default()));
                    s.set("envs", Value::Map(m.clone()));
                    m
                }
            };
            envs.borrow_mut().insert(MapKey::Str(RStr::new(key)), val);
            cmd_value()
        }
        "stdin" | "stdout" | "stderr" => {
            s.set(name, args.first().cloned().unwrap_or(Value::Unit));
            cmd_value()
        }
        "spawn" => return Ok(spawn_command(s)),
        "output" => run_command(s),
        "status" => status_command(s),
        _ => bail!("unknown method `{name}` on Command"),
    })
}

/// Methods on a spawned `Child`. Lifecycle calls delegate to the real child
/// handle; `wait_with_output` reads any piped stdout/stderr to the end first.
/// Drop the real `ChildStdin` inside a shared handle, closing the pipe. Walks a
/// `Some(Native)` wrapper from `child.stdin.take()`.
pub(super) fn close_child_stdin(v: &Value) {
    match v {
        Value::Native(rc) => *rc.borrow_mut() = Native::Closed,
        Value::Enum { enum_name, variant, data }
            if &**enum_name == "Option" && &**variant == "Some" =>
        {
            if let Some(inner) = data.first() {
                close_child_stdin(inner);
            }
        }
        _ => {}
    }
}

pub(super) fn child_method(s: &StructData, name: &str, args: &mut [Value]) -> Result<Value> {
    // Waiting on a child that was fed piped stdin must first close that pipe,
    // or the child blocks forever on EOF. Real Rust closes it when the taken
    // `ChildStdin` drops. The VM keeps every value alive in a register for the
    // whole call, so a `let w = cat.stdin.take()` clone stays live and the
    // writer never drops on its own. Close it through the shared handle instead,
    // which drops the real `ChildStdin` no matter how many clones exist.
    if matches!(name, "wait" | "wait_with_output") {
        if let Some(v) = s.get("stdin") {
            close_child_stdin(&v);
        }
        s.set("stdin", Value::none());
    }
    if name == "wait_with_output" {
        let out = drain_child_pipe(s, "stdout");
        let err = drain_child_pipe(s, "stderr");
        let status = {
            let handle = child_handle(s)?;
            let mut h = handle.borrow_mut();
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
    match native::native_method(&handle, name, args)? {
        Some(v) => Ok(v),
        None => bail!("unknown method `{name}` on Child"),
    }
}

pub(super) fn child_handle(s: &StructData) -> Result<Rc<RefCell<Native>>> {
    match s.get("handle") {
        Some(Value::Native(h)) => Ok(h),
        _ => bail!("child handle missing"),
    }
}

/// Read a child's piped stdout/stderr field to the end as a string.
pub(super) fn drain_child_pipe(s: &StructData, key: &str) -> String {
    let handle = match s.get(key) {
        Some(Value::Enum { data, .. }) => match data.first() {
            Some(Value::Native(h)) => h.clone(),
            _ => return String::new(),
        },
        _ => return String::new(),
    };
    let mut target = [Value::str("")];
    match native::native_method(&handle, "read_to_string", &mut target) {
        Ok(_) => {}
        Err(_) => return String::new(),
    }
    if let Value::Str(out) = &target[0] {
        out.to_string()
    } else {
        String::new()
    }
}

/// The `HashMap::entry` slot, without closures. Returns the stored value; for
/// container values that Rc-share, mutating the result mutates the map, so
pub(super) fn exitstatus_method(s: &StructData, name: &str) -> Result<Value> {
    Ok(match name {
        "success" => s.get("success").unwrap_or(Value::Bool(false)),
        "code" => match s.get("code") {
            Some(v) => Value::some(v),
            None => Value::none(),
        },
        _ => bail!("unknown method `{name}` on ExitStatus"),
    })
}
