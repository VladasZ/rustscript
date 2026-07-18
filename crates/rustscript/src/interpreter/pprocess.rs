//! The `Command`, `Child`, `Stdio` and pipe-reader bridge for the parallel
//! engine. Mirrors `process.rs` from the fast engine on the `Send + Sync` value
//! model, so a `#[tokio::main]` script can spawn children and stream their
//! pipes from concurrent tasks.

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::Arc;

use anyhow::{Result, bail};
use parking_lot::Mutex;

use super::pnative::PNative;
use super::pvalue::{PStructData, PValue};

/// Build a real `Command` from a script `Command` value's fields.
fn build_command(s: &Arc<PStructData>) -> std::process::Command {
    let program = s.get("program").map(|v| v.display()).unwrap_or_default();
    let mut cmd = std::process::Command::new(&program);
    if let Some(PValue::Vec(list)) = s.get("args") {
        for a in list.lock().iter() {
            cmd.arg(a.display());
        }
    }
    if let Some(dir) = s.get("current_dir")
        && !matches!(dir, PValue::Unit)
    {
        cmd.current_dir(dir.display());
    }
    if let Some(PValue::Map(envs)) = s.get("envs") {
        for (k, v) in envs.lock().iter() {
            cmd.env(k.to_value().display(), v.display());
        }
    }
    cmd
}

/// Map a stored `Stdio` marker to a real one, falling back to `default` when the
/// script never set that stream.
fn stdio_or(s: &Arc<PStructData>, key: &str, default: std::process::Stdio) -> std::process::Stdio {
    match s.get(key) {
        Some(PValue::Struct(m)) if &**m.name() == "Stdio" => {
            match m.get("kind").map(|v| v.display()).as_deref() {
                Some("piped") => std::process::Stdio::piped(),
                Some("null") => std::process::Stdio::null(),
                _ => std::process::Stdio::inherit(),
            }
        }
        _ => default,
    }
}

/// `output()` pipes by default, `status()` inherits, and an explicit setting
/// wins over either, matching real std behavior.
pub(super) fn run_command(s: &Arc<PStructData>, capture: bool) -> PValue {
    let mut cmd = build_command(s);
    if capture {
        cmd.stdin(stdio_or(s, "stdin", std::process::Stdio::null()));
        cmd.stdout(stdio_or(s, "stdout", std::process::Stdio::piped()));
        cmd.stderr(stdio_or(s, "stderr", std::process::Stdio::piped()));
        match cmd.output() {
            Ok(out) => PValue::ok(make_output(&out.stdout, &out.stderr, out.status)),
            Err(e) => PValue::err(PValue::str(e.to_string())),
        }
    } else {
        cmd.stdin(stdio_or(s, "stdin", std::process::Stdio::inherit()));
        cmd.stdout(stdio_or(s, "stdout", std::process::Stdio::inherit()));
        cmd.stderr(stdio_or(s, "stderr", std::process::Stdio::inherit()));
        match cmd.status() {
            Ok(st) => PValue::ok(exit_status(st)),
            Err(e) => PValue::err(PValue::str(e.to_string())),
        }
    }
}

/// Spawn a `Command`, returning a `Child` whose piped ends are native handles.
fn spawn_command(s: &Arc<PStructData>) -> PValue {
    let mut cmd = build_command(s);
    cmd.stdin(stdio_or(s, "stdin", std::process::Stdio::inherit()));
    cmd.stdout(stdio_or(s, "stdout", std::process::Stdio::inherit()));
    cmd.stderr(stdio_or(s, "stderr", std::process::Stdio::inherit()));
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return PValue::err(PValue::str(e.to_string())),
    };
    let stdin = child.stdin.take().map_or_else(PValue::none, |w| {
        PValue::some(PNative::ChildStdin(w).wrap())
    });
    let stdout = child.stdout.take().map_or_else(PValue::none, reader_value);
    let stderr = child.stderr.take().map_or_else(PValue::none, reader_value);
    PValue::ok(PValue::struct_of(
        "Child",
        [
            ("handle".into(), PNative::Child(child).wrap()),
            ("stdin".into(), stdin),
            ("stdout".into(), stdout),
            ("stderr".into(), stderr),
        ],
    ))
}

fn reader_value(r: impl Read + Send + 'static) -> PValue {
    PValue::some(PNative::Reader(BufReader::new(Box::new(r) as Box<dyn Read + Send>)).wrap())
}

pub(super) fn exit_status(status: std::process::ExitStatus) -> PValue {
    PValue::struct_of(
        "ExitStatus",
        [
            ("success".into(), PValue::Bool(status.success())),
            (
                "code".into(),
                PValue::Int(i64::from(status.code().unwrap_or(-1))),
            ),
        ],
    )
}

fn make_output(stdout: &[u8], stderr: &[u8], status: std::process::ExitStatus) -> PValue {
    PValue::struct_of(
        "Output",
        [
            ("status".into(), exit_status(status)),
            ("stdout".into(), byte_vec(stdout)),
            ("stderr".into(), byte_vec(stderr)),
        ],
    )
}

fn byte_vec(bytes: &[u8]) -> PValue {
    PValue::vec(bytes.iter().map(|&b| PValue::Int(i64::from(b))).collect())
}

pub(super) fn command_method(recv: &PValue, m: &str, args: &mut [PValue]) -> Result<PValue> {
    let PValue::Struct(s) = recv else {
        unreachable!()
    };
    Ok(match m {
        "arg" => {
            push_arg(s, args.first().cloned().unwrap_or(PValue::Unit));
            recv.clone()
        }
        "args" => {
            if let Some(PValue::Vec(list)) = args.first() {
                for a in list.lock().iter() {
                    push_arg(s, a.clone());
                }
            }
            recv.clone()
        }
        "current_dir" => {
            s.set("current_dir", args.first().cloned().unwrap_or(PValue::Unit));
            recv.clone()
        }
        "env" => {
            let key = args.first().map(PValue::display).unwrap_or_default();
            let val = args.get(1).cloned().unwrap_or(PValue::Unit);
            let envs = match s.get("envs") {
                Some(PValue::Map(m)) => PValue::Map(m),
                _ => {
                    let m = PValue::map();
                    s.set("envs", m.clone());
                    m
                }
            };
            if let PValue::Map(map) = envs
                && let Some(k) = PValue::str(key).as_key()
            {
                map.lock().insert(k, val);
            }
            recv.clone()
        }
        "stdin" | "stdout" | "stderr" => {
            s.set(m, args.first().cloned().unwrap_or(PValue::Unit));
            recv.clone()
        }
        "spawn" => spawn_command(s),
        "status" => run_command(s, false),
        "output" => run_command(s, true),
        _ => bail!("method `{m}` on Command is not supported in tokio mode"),
    })
}

fn push_arg(s: &Arc<PStructData>, a: PValue) {
    if let Some(PValue::Vec(list)) = s.get("args") {
        list.lock().push(PValue::str(a.display()));
    }
}

/// Methods on a spawned `Child`.
pub(super) fn child_method(recv: &PValue, m: &str) -> Result<PValue> {
    let PValue::Struct(s) = recv else {
        unreachable!()
    };
    // Waiting on a child fed through a piped stdin must close that pipe first or
    // the child blocks forever waiting for EOF. Real Rust closes it when the
    // taken `ChildStdin` drops, but the VM keeps every value alive in a register
    // for the whole call, so drop it through the shared handle instead.
    if matches!(m, "wait" | "wait_with_output") {
        if let Some(v) = s.get("stdin") {
            close_handle(&v);
        }
        s.set("stdin", PValue::none());
    }
    match m {
        "wait" => {
            let status = wait_child(s)?;
            Ok(match status {
                Ok(st) => PValue::ok(exit_status(st)),
                Err(e) => PValue::err(PValue::str(e)),
            })
        }
        "wait_with_output" => {
            // Drain before waiting, so a child that fills its pipe buffer is not
            // deadlocked against a parent that is waiting for it to exit.
            let out = drain_pipe(s, "stdout");
            let err = drain_pipe(s, "stderr");
            Ok(match wait_child(s)? {
                Ok(st) => PValue::ok(make_output(out.as_bytes(), err.as_bytes(), st)),
                Err(e) => PValue::err(PValue::str(e)),
            })
        }
        "id" => {
            let handle = child_handle(s)?;
            let mut h = handle.lock();
            match &mut *h {
                PNative::Child(c) => Ok(PValue::Int(i64::from(c.id()))),
                _ => bail!("child handle missing"),
            }
        }
        "kill" => {
            let handle = child_handle(s)?;
            let mut h = handle.lock();
            match &mut *h {
                PNative::Child(c) => Ok(match c.kill() {
                    Ok(()) => PValue::ok(PValue::Unit),
                    Err(e) => PValue::err(PValue::str(e.to_string())),
                }),
                _ => bail!("child handle missing"),
            }
        }
        _ => bail!("method `{m}` on Child is not supported in tokio mode"),
    }
}

type WaitResult = Result<std::result::Result<std::process::ExitStatus, String>>;

fn wait_child(s: &Arc<PStructData>) -> WaitResult {
    let handle = child_handle(s)?;
    let mut h = handle.lock();
    match &mut *h {
        PNative::Child(c) => Ok(c.wait().map_err(|e| e.to_string())),
        _ => bail!("child handle missing"),
    }
}

fn child_handle(s: &Arc<PStructData>) -> Result<Arc<Mutex<PNative>>> {
    match s.get("handle") {
        Some(PValue::Native(h)) => Ok(h),
        _ => bail!("child handle missing"),
    }
}

/// Replace a handle with `Taken`, dropping the real resource it held. Walks the
/// `Some(..)` wrapper a child's stdin field carries.
fn close_handle(v: &PValue) {
    match v {
        PValue::Native(h) => *h.lock() = PNative::Taken,
        PValue::Enum { variant, data, .. } if &**variant == "Some" => {
            if let Some(inner) = data.first() {
                close_handle(inner);
            }
        }
        _ => {}
    }
}

/// Read a child's piped stdout or stderr to the end.
fn drain_pipe(s: &Arc<PStructData>, key: &str) -> String {
    let Some(PValue::Enum { data, .. }) = s.get(key) else {
        return String::new();
    };
    let Some(PValue::Native(h)) = data.first() else {
        return String::new();
    };
    let mut buf = String::new();
    if let Some(r) = h.lock().as_read() {
        let _read = r.read_to_string(&mut buf);
    }
    buf
}

/// Reader, line iterator and stdin-writer methods on a native handle.
pub(super) fn native_method(
    handle: &Arc<Mutex<PNative>>,
    m: &str,
    args: &mut [PValue],
) -> Result<Option<PValue>> {
    match m {
        "lines" => {
            // Move the reader out into a lazy iterator so a for-loop streams the
            // pipe instead of buffering all of it first. The original is emptied.
            let taken = std::mem::replace(&mut *handle.lock(), PNative::Taken);
            let PNative::Reader(r) = taken else {
                *handle.lock() = taken;
                bail!("lines on a non-reader handle");
            };
            Ok(Some(PNative::Lines(Box::new(r.lines())).wrap()))
        }
        "next" => {
            let mut h = handle.lock();
            let PNative::Lines(iter) = &mut *h else {
                bail!("next on a non-iterator handle");
            };
            Ok(Some(match iter.next() {
                Some(Ok(line)) => PValue::some(PValue::ok(PValue::str(line))),
                Some(Err(e)) => PValue::some(PValue::err(PValue::str(e.to_string()))),
                None => PValue::none(),
            }))
        }
        "read_line" => {
            let mut h = handle.lock();
            let Some(r) = h.as_read() else {
                bail!("read_line on a non-reader handle");
            };
            let mut buf = String::new();
            let mut reader = BufReader::new(r);
            Ok(Some(match reader.read_line(&mut buf) {
                Ok(n) => {
                    if let Some(t) = args.first_mut() {
                        *t = PValue::str(buf);
                    }
                    PValue::ok(PValue::Int(n as i64))
                }
                Err(e) => PValue::err(PValue::str(e.to_string())),
            }))
        }
        "read_to_string" => {
            let mut h = handle.lock();
            let Some(r) = h.as_read() else {
                bail!("read_to_string on a non-reader handle");
            };
            let mut buf = String::new();
            Ok(Some(match r.read_to_string(&mut buf) {
                Ok(n) => {
                    if let Some(t) = args.first_mut() {
                        *t = PValue::str(buf);
                    }
                    PValue::ok(PValue::Int(n as i64))
                }
                Err(e) => PValue::err(PValue::str(e.to_string())),
            }))
        }
        "write_all" | "write" => {
            let mut h = handle.lock();
            let PNative::ChildStdin(w) = &mut *h else {
                bail!("write on a non-writer handle");
            };
            let data = args.first().map(PValue::display).unwrap_or_default();
            Ok(Some(match w.write_all(data.as_bytes()) {
                Ok(()) => PValue::ok(PValue::Unit),
                Err(e) => PValue::err(PValue::str(e.to_string())),
            }))
        }
        "flush" => {
            let mut h = handle.lock();
            let PNative::ChildStdin(w) = &mut *h else {
                bail!("flush on a non-writer handle");
            };
            Ok(Some(match w.flush() {
                Ok(()) => PValue::ok(PValue::Unit),
                Err(e) => PValue::err(PValue::str(e.to_string())),
            }))
        }
        _ => Ok(None),
    }
}
