//! The `Command`, `Child`, and process output bridge.
//! Split from `builtins.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Result, bail};

use super::native::{self, Native};

use super::value::{Fields, Map, MapKey, RStr, Value};



/// Build a real `Command` from a script `Command` value's fields.
pub(super) fn build_command(f: &Fields) -> std::process::Command {
    let program = f.get("program").map(|v| v.display()).unwrap_or_default();
    let mut cmd = std::process::Command::new(&program);
    if let Some(Value::Vec(a)) = f.get("args") {
        for item in a.borrow().iter() {
            cmd.arg(item.display());
        }
    }
    if let Some(cwd) = f.get("cwd") {
        cmd.current_dir(cwd.display());
    }
    if let Some(Value::Map(envs)) = f.get("envs") {
        for (k, v) in envs.borrow().iter() {
            cmd.env(k.to_value().display(), v.display());
        }
    }
    cmd
}

/// Run a `Command` value once it has been fully built, returning an `Output`.
pub(super) fn run_command(fields: &Rc<RefCell<Fields>>) -> Value {
    let f = fields.borrow();
    match build_command(&f).output() {
        Ok(out) => Value::ok(make_output(out)),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

/// Map a stored `Stdio` marker to a real `std::process::Stdio`, defaulting to
/// inherit so a spawned child shares the terminal like a shell command.
pub(super) fn stdio_for(f: &Fields, key: &str) -> std::process::Stdio {
    match f.get(key) {
        Some(Value::Struct { name, fields }) if &**name == "Stdio" => {
            match fields.borrow().get("kind").map(|v| v.display()).as_deref() {
                Some("piped") => std::process::Stdio::piped(),
                Some("null") => std::process::Stdio::null(),
                _ => std::process::Stdio::inherit(),
            }
        }
        _ => std::process::Stdio::inherit(),
    }
}

/// Spawn a `Command`, returning a `Child` value whose stdin/stdout/stderr
/// fields hold the piped ends as native handles.
pub(super) fn spawn_command(fields: &Rc<RefCell<Fields>>) -> Value {
    let f = fields.borrow();
    let mut cmd = build_command(&f);
    cmd.stdin(stdio_for(&f, "stdin"));
    cmd.stdout(stdio_for(&f, "stdout"));
    cmd.stderr(stdio_for(&f, "stderr"));
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
    let mut cf = Fields::default();
    cf.insert("handle".into(), Native::Child(child).wrap());
    cf.insert("stdin".into(), stdin);
    cf.insert("stdout".into(), stdout);
    cf.insert("stderr".into(), stderr);
    Value::ok(Value::Struct {
        name: "Child".into(),
        fields: Rc::new(RefCell::new(cf)),
    })
}

/// Build an `ExitStatus` value with `code` and `success`.
pub(super) fn make_exit_status(status: std::process::ExitStatus) -> Value {
    let mut st = Fields::default();
    st.insert("code".into(), Value::Int(status.code().unwrap_or(-1) as i64));
    st.insert("success".into(), Value::Bool(status.success()));
    Value::Struct {
        name: "ExitStatus".into(),
        fields: Rc::new(RefCell::new(st)),
    }
}

/// Build an `Output` value with `stdout`, `stderr`, and `status`.
pub(super) fn make_output(out: std::process::Output) -> Value {
    let mut o = Fields::default();
    o.insert(
        "stdout".into(),
        Value::str(String::from_utf8_lossy(&out.stdout).into_owned()),
    );
    o.insert(
        "stderr".into(),
        Value::str(String::from_utf8_lossy(&out.stderr).into_owned()),
    );
    o.insert("status".into(), make_exit_status(out.status));
    Value::Struct {
        name: "Output".into(),
        fields: Rc::new(RefCell::new(o)),
    }
}

pub(super) fn command_method(
    fields: &Rc<RefCell<Fields>>,
    name: &str,
    args: &[Value],
) -> Result<Value> {
    let cmd_value = || Value::Struct {
        name: "Command".into(),
        fields: fields.clone(),
    };
    Ok(match name {
        "arg" => {
            if let Some(Value::Vec(list)) = fields.borrow().get("args") {
                list.borrow_mut()
                    .push(args.first().cloned().unwrap_or(Value::Unit));
            }
            cmd_value()
        }
        "args" => {
            if let (Some(Value::Vec(list)), Some(Value::Vec(extra))) =
                (fields.borrow().get("args"), args.first())
            {
                list.borrow_mut().extend(extra.borrow().iter().cloned());
            }
            cmd_value()
        }
        "current_dir" => {
            fields
                .borrow_mut()
                .insert("cwd".into(), args.first().cloned().unwrap_or(Value::Unit));
            cmd_value()
        }
        "env" => {
            let mut f = fields.borrow_mut();
            let key = args.first().map(|v| v.display()).unwrap_or_default();
            let val = args.get(1).cloned().unwrap_or(Value::Unit);
            let entry = f
                .entry("envs".into())
                .or_insert_with(|| Value::Map(Rc::new(RefCell::new(Map::default()))));
            if let Value::Map(m) = entry {
                m.borrow_mut().insert(MapKey::Str(RStr::new(key)), val);
            }
            drop(f);
            cmd_value()
        }
        "stdin" | "stdout" | "stderr" => {
            fields
                .borrow_mut()
                .insert(name.into(), args.first().cloned().unwrap_or(Value::Unit));
            cmd_value()
        }
        "spawn" => return Ok(spawn_command(fields)),
        "output" => run_command(fields),
        "status" => match run_command(fields) {
            Value::Enum { data, .. } => {
                let out = data.first().cloned().unwrap_or(Value::Unit);
                match out {
                    Value::Struct { fields: of, .. } => {
                        Value::ok(of.borrow().get("status").cloned().unwrap_or(Value::Unit))
                    }
                    other => Value::ok(other),
                }
            }
            other => other,
        },
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

pub(super) fn child_method(fields: &Rc<RefCell<Fields>>, name: &str, args: &mut [Value]) -> Result<Value> {
    // Waiting on a child that was fed piped stdin must first close that pipe,
    // or the child blocks forever on EOF. Real Rust closes it when the taken
    // `ChildStdin` drops. The VM keeps every value alive in a register for the
    // whole call, so a `let w = cat.stdin.take()` clone stays live and the
    // writer never drops on its own. Close it through the shared handle instead,
    // which drops the real `ChildStdin` no matter how many clones exist.
    if matches!(name, "wait" | "wait_with_output") {
        let stdin_val = fields.borrow().get("stdin").cloned();
        if let Some(v) = stdin_val {
            close_child_stdin(&v);
        }
        if let Some(slot) = fields.borrow_mut().get_mut("stdin") {
            *slot = Value::none();
        }
    }
    if name == "wait_with_output" {
        let out = drain_child_pipe(fields, "stdout");
        let err = drain_child_pipe(fields, "stderr");
        let status = {
            let handle = child_handle(fields)?;
            let mut h = handle.borrow_mut();
            if let Native::Child(c) = &mut *h {
                match c.wait() {
                    Ok(s) => s,
                    Err(e) => return Ok(Value::err(Value::str(e.to_string()))),
                }
            } else {
                bail!("child handle missing");
            }
        };
        let mut o = Fields::default();
        o.insert("stdout".into(), Value::str(out));
        o.insert("stderr".into(), Value::str(err));
        o.insert("status".into(), make_exit_status(status));
        return Ok(Value::ok(Value::Struct {
            name: "Output".into(),
            fields: Rc::new(RefCell::new(o)),
        }));
    }
    let handle = child_handle(fields)?;
    match native::native_method(&handle, name, args)? {
        Some(v) => Ok(v),
        None => bail!("unknown method `{name}` on Child"),
    }
}

pub(super) fn child_handle(fields: &Rc<RefCell<Fields>>) -> Result<Rc<RefCell<Native>>> {
    match fields.borrow().get("handle") {
        Some(Value::Native(h)) => Ok(h.clone()),
        _ => bail!("child handle missing"),
    }
}

/// Read a child's piped stdout/stderr field to the end as a string.
pub(super) fn drain_child_pipe(fields: &Rc<RefCell<Fields>>, key: &str) -> String {
    let handle = match fields.borrow().get(key) {
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
    if let Value::Str(s) = &target[0] {
        s.to_string()
    } else {
        String::new()
    }
}

/// The `HashMap::entry` slot, without closures. Returns the stored value; for
/// container values that Rc-share, mutating the result mutates the map, so
pub(super) fn exitstatus_method(fields: &Rc<RefCell<Fields>>, name: &str) -> Result<Value> {
    let f = fields.borrow();
    Ok(match name {
        "success" => f.get("success").cloned().unwrap_or(Value::Bool(false)),
        "code" => match f.get("code") {
            Some(v) => Value::some(v.clone()),
            None => Value::none(),
        },
        _ => bail!("unknown method `{name}` on ExitStatus"),
    })
}
