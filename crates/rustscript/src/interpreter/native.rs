//! Live host resources that cannot be rebuilt from plain field values.
//!
//! A `File`, `Child`, `TcpStream`, or buffered reader owns an OS handle. Unlike
//! `Regex` or `Command`, which the interpreter stores as struct fields and
//! rebuilds on demand, these must be kept alive as real Rust values. They live
//! behind `Value::Native(Rc<RefCell<Native>>)` so a script can share and mutate
//! one, matching how real Rust passes these handles around.

use std::cell::RefCell;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, ChildStdin};
use std::rc::Rc;
use std::time::{Instant, SystemTime};

use anyhow::{Result, bail};

use super::value::Value;

/// A boxed reader for stdin, a child's stdout/stderr, or a socket. Files keep
/// their own variant so they can also write and seek.
pub enum Native {
    File(BufReader<File>),
    Reader(BufReader<Box<dyn Read>>),
    Writer(Box<dyn Write>),
    Child(Child),
    ChildStdin(ChildStdin),
    Listener(TcpListener),
    Stream(TcpStream),
    Instant(Instant),
    SystemTime(SystemTime),
    TempDir(tempfile::TempDir),
    Agent(ureq::Agent),
    /// A lazy line iterator, so `for line in reader.lines()` streams instead of
    /// buffering the whole input first.
    Lines(Box<dyn Iterator<Item = std::io::Result<String>>>),
    /// A handle that has been force dropped, used to close a child's stdin pipe
    /// even while another register still holds a reference to it.
    Closed,
}

impl Native {
    pub fn wrap(self) -> Value {
        Value::Native(Rc::new(RefCell::new(self)))
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Native::File(_) => "File",
            Native::Reader(_) => "Reader",
            Native::Writer(_) => "Writer",
            Native::Child(_) => "Child",
            Native::ChildStdin(_) => "ChildStdin",
            Native::Listener(_) => "TcpListener",
            Native::Stream(_) => "TcpStream",
            Native::Instant(_) => "Instant",
            Native::SystemTime(_) => "SystemTime",
            Native::TempDir(_) => "TempDir",
            Native::Agent(_) => "Agent",
            Native::Lines(_) => "Lines",
            Native::Closed => "Closed",
        }
    }
}

/// Pull the next line from a lazy `Lines` iterator, `None` at end of input.
/// Each item is a `Result<String>` so a script can use `line?` in the loop.
pub fn lines_next(handle: &Rc<RefCell<Native>>) -> Option<Value> {
    let mut h = handle.borrow_mut();
    if let Native::Lines(it) = &mut *h {
        match it.next() {
            Some(Ok(line)) => Some(Value::ok(Value::str(line))),
            Some(Err(e)) => Some(Value::err(Value::str(e.to_string()))),
            None => None,
        }
    } else {
        None
    }
}

/// Drain a lazy `Lines` iterator fully, for `.collect()` or a materializing
/// `for` loop. Only meaningful on `Native::Lines`.
pub fn drain_lines(handle: &Rc<RefCell<Native>>) -> Vec<Value> {
    let mut out = Vec::new();
    while let Some(v) = lines_next(handle) {
        out.push(v);
    }
    out
}

fn io_err<T>(r: std::io::Result<T>, on_ok: impl FnOnce(T) -> Value) -> Value {
    match r {
        Ok(v) => Value::ok(on_ok(v)),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

/// Read a reader as one of the common shapes. Returns None when the handle is
/// not a reader.
fn as_read(h: &mut Native) -> Option<&mut dyn BufRead> {
    match h {
        Native::File(r) => Some(r),
        Native::Reader(r) => Some(r),
        _ => None,
    }
}

fn append_string(target: &Value, text: &str) {
    if let Value::Str(s) = target {
        s.borrow_mut().push_str(text);
    }
}

fn append_bytes(target: &Value, bytes: &[u8]) {
    if let Value::Vec(v) = target {
        v.borrow_mut()
            .extend(bytes.iter().map(|b| Value::Int(*b as i64)));
    }
}

/// Dispatch a method call on a native handle. Returns `Ok(None)` when the
/// method is unknown for this handle so the caller can raise a good error.
pub fn native_method(
    handle: &Rc<RefCell<Native>>,
    method: &str,
    args: &[Value],
) -> Result<Option<Value>> {
    // Handles that consume self or hand out sub-handles need to move out of the
    // RefCell, so they are matched first with a dedicated borrow.
    match method {
        // Reader side ------------------------------------------------------
        "read_line" => {
            let target = args.first().cloned().unwrap_or(Value::Unit);
            let mut h = handle.borrow_mut();
            let Some(r) = as_read(&mut h) else {
                bail!("read_line on non-reader {}", h.type_name());
            };
            let mut buf = String::new();
            return Ok(Some(io_err(r.read_line(&mut buf), |n| {
                append_string(&target, &buf);
                Value::Int(n as i64)
            })));
        }
        "read_to_string" => {
            let target = args.first().cloned().unwrap_or(Value::Unit);
            let mut h = handle.borrow_mut();
            let Some(r) = as_read(&mut h) else {
                bail!("read_to_string on non-reader {}", h.type_name());
            };
            let mut buf = String::new();
            return Ok(Some(io_err(r.read_to_string(&mut buf), |n| {
                append_string(&target, &buf);
                Value::Int(n as i64)
            })));
        }
        "read_to_end" => {
            let target = args.first().cloned().unwrap_or(Value::Unit);
            let mut h = handle.borrow_mut();
            let Some(r) = as_read(&mut h) else {
                bail!("read_to_end on non-reader {}", h.type_name());
            };
            let mut buf = Vec::new();
            return Ok(Some(io_err(r.read_to_end(&mut buf), |n| {
                append_bytes(&target, &buf);
                Value::Int(n as i64)
            })));
        }
        "lines" => {
            // Move the reader out into a lazy line iterator so a for-loop can
            // stream it. The original handle is left empty.
            let taken = std::mem::replace(&mut *handle.borrow_mut(), Native::Lines(Box::new(std::iter::empty())));
            let iter: Box<dyn Iterator<Item = std::io::Result<String>>> = match taken {
                Native::File(r) => Box::new(r.lines()),
                Native::Reader(r) => Box::new(r.lines()),
                other => {
                    *handle.borrow_mut() = other;
                    bail!("lines on non-reader");
                }
            };
            return Ok(Some(Native::Lines(iter).wrap()));
        }
        "next" => {
            return Ok(Some(match lines_next(handle) {
                Some(v) => Value::some(v),
                None => Value::none(),
            }));
        }
        "collect" => {
            if matches!(&*handle.borrow(), Native::Lines(_)) {
                return Ok(Some(Value::vec(drain_lines(handle))));
            }
        }
        // Writer side ------------------------------------------------------
        "write_all" | "write" => {
            let bytes = value_to_bytes(args.first());
            let mut h = handle.borrow_mut();
            let n = bytes.len();
            let r = write_bytes(&mut h, &bytes);
            let is_write = method == "write";
            return Ok(Some(io_err(r, |()| {
                if is_write {
                    Value::Int(n as i64)
                } else {
                    Value::Unit
                }
            })));
        }
        "flush" => {
            let mut h = handle.borrow_mut();
            let r = flush_writer(&mut h);
            return Ok(Some(io_err(r, |()| Value::Unit)));
        }
        // File extras ------------------------------------------------------
        "seek" => {
            let pos = seek_from(args.first());
            let mut h = handle.borrow_mut();
            if let Native::File(r) = &mut *h {
                return Ok(Some(io_err(r.seek(pos), |n| Value::Int(n as i64))));
            }
            bail!("seek on non-file {}", h.type_name());
        }
        "sync_all" | "sync_data" => {
            let mut h = handle.borrow_mut();
            if let Native::File(r) = &mut *h {
                return Ok(Some(io_err(r.get_ref().sync_all(), |()| Value::Unit)));
            }
            bail!("sync on non-file {}", h.type_name());
        }
        "set_len" => {
            let n = as_int(args.first()).unwrap_or(0) as u64;
            let mut h = handle.borrow_mut();
            if let Native::File(r) = &mut *h {
                return Ok(Some(io_err(r.get_ref().set_len(n), |()| Value::Unit)));
            }
            bail!("set_len on non-file {}", h.type_name());
        }
        "metadata" => {
            let h = handle.borrow();
            if let Native::File(r) = &*h {
                return Ok(Some(io_err(r.get_ref().metadata(), |m| {
                    super::builtins::make_metadata(&m)
                })));
            }
            bail!("metadata on non-file {}", h.type_name());
        }
        // Child ------------------------------------------------------------
        "wait" => {
            let mut h = handle.borrow_mut();
            if let Native::Child(c) = &mut *h {
                return Ok(Some(io_err(c.wait(), |s| {
                    super::builtins::make_exit_status(s)
                })));
            }
            bail!("wait on non-child {}", h.type_name());
        }
        "try_wait" => {
            let mut h = handle.borrow_mut();
            if let Native::Child(c) = &mut *h {
                return Ok(Some(match c.try_wait() {
                    Ok(Some(s)) => Value::ok(Value::some(super::builtins::make_exit_status(s))),
                    Ok(None) => Value::ok(Value::none()),
                    Err(e) => Value::err(Value::str(e.to_string())),
                }));
            }
            bail!("try_wait on non-child {}", h.type_name());
        }
        "kill" => {
            let mut h = handle.borrow_mut();
            if let Native::Child(c) = &mut *h {
                return Ok(Some(io_err(c.kill(), |()| Value::Unit)));
            }
            bail!("kill on non-child {}", h.type_name());
        }
        "id" => {
            let h = handle.borrow();
            if let Native::Child(c) = &*h {
                return Ok(Some(Value::Int(c.id() as i64)));
            }
        }
        "wait_with_output" => {
            let taken = std::mem::replace(&mut *handle.borrow_mut(), Native::Lines(Box::new(std::iter::empty())));
            if let Native::Child(c) = taken {
                return Ok(Some(match c.wait_with_output() {
                    Ok(o) => Value::ok(super::builtins::make_output(o)),
                    Err(e) => Value::err(Value::str(e.to_string())),
                }));
            }
            bail!("wait_with_output on non-child");
        }
        // TcpListener ------------------------------------------------------
        "accept" => {
            let h = handle.borrow();
            if let Native::Listener(l) = &*h {
                return Ok(Some(match l.accept() {
                    Ok((stream, addr)) => Value::ok(Value::Tuple(Rc::new(RefCell::new(vec![
                        Native::Stream(stream).wrap(),
                        Value::str(addr.to_string()),
                    ])))),
                    Err(e) => Value::err(Value::str(e.to_string())),
                }));
            }
            bail!("accept on non-listener {}", h.type_name());
        }
        "incoming" => {
            // Materialize an unbounded acceptor is impossible; scripts loop with
            // accept(). Provide incoming() as a single-accept lazy iterator so a
            // `for stream in listener.incoming()` still works forever.
            bail!("incoming() is not supported; loop with listener.accept() instead");
        }
        "local_addr" => {
            let h = handle.borrow();
            let addr = match &*h {
                Native::Listener(l) => l.local_addr(),
                Native::Stream(s) => s.local_addr(),
                _ => bail!("local_addr on {}", h.type_name()),
            };
            return Ok(Some(io_err(addr, |a| Value::str(a.to_string()))));
        }
        "peer_addr" => {
            let h = handle.borrow();
            if let Native::Stream(s) = &*h {
                return Ok(Some(io_err(s.peer_addr(), |a| Value::str(a.to_string()))));
            }
            bail!("peer_addr on {}", h.type_name());
        }
        "shutdown" => {
            let h = handle.borrow();
            if let Native::Stream(s) = &*h {
                return Ok(Some(io_err(
                    s.shutdown(std::net::Shutdown::Both),
                    |()| Value::Unit,
                )));
            }
            bail!("shutdown on {}", h.type_name());
        }
        "try_clone" => {
            let h = handle.borrow();
            match &*h {
                Native::Stream(s) => {
                    return Ok(Some(io_err(s.try_clone(), |s| Native::Stream(s).wrap())));
                }
                _ => bail!("try_clone on {}", h.type_name()),
            }
        }
        // Instant / SystemTime --------------------------------------------
        "elapsed" => {
            let h = handle.borrow();
            match &*h {
                Native::Instant(t) => {
                    return Ok(Some(super::builtins::make_duration(t.elapsed())));
                }
                Native::SystemTime(t) => {
                    return Ok(Some(match t.elapsed() {
                        Ok(d) => Value::ok(super::builtins::make_duration(d)),
                        Err(e) => Value::err(Value::str(e.to_string())),
                    }));
                }
                _ => bail!("elapsed on {}", h.type_name()),
            }
        }
        "duration_since" => {
            let h = handle.borrow();
            match (&*h, args.first()) {
                (Native::Instant(t), Some(Value::Native(other))) => {
                    if let Native::Instant(o) = &*other.borrow() {
                        return Ok(Some(super::builtins::make_duration(t.duration_since(*o))));
                    }
                }
                (Native::SystemTime(t), Some(Value::Native(other))) => {
                    if let Native::SystemTime(o) = &*other.borrow() {
                        return Ok(Some(match t.duration_since(*o) {
                            Ok(d) => Value::ok(super::builtins::make_duration(d)),
                            Err(e) => Value::err(Value::str(e.to_string())),
                        }));
                    }
                }
                _ => {}
            }
            bail!("duration_since arguments mismatch");
        }
        // TempDir ----------------------------------------------------------
        "path" => {
            let h = handle.borrow();
            if let Native::TempDir(d) = &*h {
                return Ok(Some(super::builtins::make_path(d.path().display().to_string())));
            }
        }
        "close" => {
            let taken = std::mem::replace(&mut *handle.borrow_mut(), Native::Lines(Box::new(std::iter::empty())));
            if let Native::TempDir(d) = taken {
                return Ok(Some(io_err(d.close(), |()| Value::Unit)));
            }
            bail!("close on non-tempdir");
        }
        // Agent request builders ------------------------------------------
        "get" | "post" | "put" | "delete" | "patch" | "head" => {
            if matches!(&*handle.borrow(), Native::Agent(_)) {
                let verb = method.to_ascii_uppercase();
                let agent = Value::Native(handle.clone());
                return Ok(Some(super::builtins::build_http_request(
                    &verb,
                    args.first(),
                    Some(agent),
                )));
            }
        }
        _ => {}
    }
    Ok(None)
}

fn write_bytes(h: &mut Native, bytes: &[u8]) -> std::io::Result<()> {
    match h {
        Native::File(r) => r.get_mut().write_all(bytes),
        Native::Writer(w) => w.write_all(bytes),
        Native::ChildStdin(w) => w.write_all(bytes),
        Native::Stream(s) => s.write_all(bytes),
        other => Err(std::io::Error::other(format!(
            "cannot write to {}",
            other.type_name()
        ))),
    }
}

fn flush_writer(h: &mut Native) -> std::io::Result<()> {
    match h {
        Native::File(r) => r.get_mut().flush(),
        Native::Writer(w) => w.flush(),
        Native::ChildStdin(w) => w.flush(),
        Native::Stream(s) => s.flush(),
        _ => Ok(()),
    }
}

fn value_to_bytes(v: Option<&Value>) -> Vec<u8> {
    match v {
        Some(Value::Str(s)) => s.borrow().clone().into_bytes(),
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

fn as_int(v: Option<&Value>) -> Option<i64> {
    match v {
        Some(Value::Int(i)) => Some(*i),
        _ => None,
    }
}

fn seek_from(v: Option<&Value>) -> SeekFrom {
    // A script passes SeekFrom::Start(n) etc., which the interpreter models as
    // an enum value carrying the offset.
    if let Some(Value::Enum { variant, data, .. }) = v {
        let n = data.borrow().first().and_then(|x| match x {
            Value::Int(i) => Some(*i),
            _ => None,
        });
        match (variant.as_str(), n) {
            ("Start", Some(n)) => return SeekFrom::Start(n as u64),
            ("End", Some(n)) => return SeekFrom::End(n as i64),
            ("Current", Some(n)) => return SeekFrom::Current(n as i64),
            _ => {}
        }
    }
    SeekFrom::Current(0)
}
