//! Methods on live host resources behind `Value::Native`.

use std::io::{Seek, SeekFrom, Write};
use std::sync::Arc;

use anyhow::{Result, bail};
use parking_lot::Mutex;

use super::bytecode::{BuiltinId, MethodName};
use super::enum_def::ERROR_KIND;
use super::native::Native;
use super::value::Value;

type Handle = Arc<Mutex<Native>>;

/// Each item is a `Result<String>` so a script can use `line?`.
pub(super) fn lines_next(handle: &Handle) -> Option<Value> {
    let mut h = handle.lock();
    if let Native::Lines(it) = &mut *h {
        match it.next() {
            Some(Ok(line)) => Some(Value::ok(Value::str(line))),
            Some(Err(e)) => Some(Value::err(super::native::io_error_value(&e))),
            None => None,
        }
    } else {
        None
    }
}

pub(super) fn drain_lines(handle: &Handle) -> Vec<Value> {
    let mut out = Vec::new();
    while let Some(v) = lines_next(handle) {
        out.push(v);
    }
    out
}

fn int_len(n: usize) -> i64 {
    i64::try_from(n).expect("length exceeds i64")
}

fn io_err<T>(r: std::io::Result<T>, on_ok: impl FnOnce(T) -> Value) -> Value {
    match r {
        Ok(v) => Value::ok(on_ok(v)),
        Err(e) => Value::err(super::native::io_error_value(&e)),
    }
}

/// The buffer arrives as a copy, the vm moves it back into the variable after the call, see
/// `compile_method`.
fn append_string(target: &mut Value, text: &str) {
    if let Value::Str(s) = target {
        let mut out = s.to_string();
        out.push_str(text);
        *target = Value::str(out);
    }
}

/// Written as `b'\n'` or as a plain integer.
fn byte_arg(arg: Option<&Value>, method: &str) -> Result<u8> {
    let Some(Value::Int(n)) = arg else {
        bail!("{method} needs a byte as its first argument");
    };
    match u8::try_from(*n) {
        Ok(b) => Ok(b),
        Err(_) => bail!("{method} got {n}, which is not a byte"),
    }
}

fn append_bytes(target: &Value, bytes: &[u8]) {
    if let Value::Vec(v) = target {
        v.lock()
            .extend(bytes.iter().map(|b| Value::Int(i64::from(*b))));
    }
}

/// `Ok(None)` when the method is unknown for this handle.
pub(super) fn native_method(
    handle: &Handle,
    method: &MethodName,
    args: &mut [Value],
) -> Result<Option<Value>> {
    // a lopdf Document dispatches by receiver first, its names must not collide with the name
    // keyed arms below
    if matches!(&*handle.lock(), Native::Pdf(_)) {
        let mut h = handle.lock();
        let Native::Pdf(doc) = &mut *h else {
            unreachable!()
        };
        if let Some(v) = super::pdf_bridge::document_method(doc, method, args)? {
            return Ok(Some(v));
        }
    }
    if let Some(v) = super::crates_bridge::sha256_method(handle, method, args)? {
        return Ok(Some(v));
    }
    if let Some(v) = super::ed25519_bridge::signing_key_method(handle, method, args)? {
        return Ok(Some(v));
    }
    if let Some(v) = super::ed25519_bridge::verifying_key_method(handle, method, args)? {
        return Ok(Some(v));
    }
    if let Some(v) = super::ed25519_bridge::signature_method(handle, method)? {
        return Ok(Some(v));
    }
    if let Some(v) = io_error_method(handle, method) {
        return Ok(Some(v));
    }
    if let Some(v) = joinerr_method(handle, method) {
        return Ok(Some(v));
    }
    // the families use disjoint names, handles that consume self move out of the Mutex inside
    // their helper
    if let Some(v) = reader_native_method(handle, method, args)? {
        return Ok(Some(v));
    }
    if let Some(v) = writer_native_method(handle, method, args) {
        return Ok(Some(v));
    }
    if let Some(v) = file_native_method(handle, method, args)? {
        return Ok(Some(v));
    }
    if let Some(v) = child_native_method(handle, method)? {
        return Ok(Some(v));
    }
    if let Some(v) = net_native_method(handle, method)? {
        return Ok(Some(v));
    }
    if let Some(v) = udp_native_method(handle, method, args)? {
        return Ok(Some(v));
    }
    if let Some(v) = time_native_method(handle, method, args)? {
        return Ok(Some(v));
    }
    temp_native_method(handle, method)
}

pub(super) fn io_error_method(handle: &Handle, method: &MethodName) -> Option<Value> {
    let h = handle.lock();
    let Native::IoErr { kind, code, .. } = &*h else {
        return None;
    };
    match method.id {
        BuiltinId::Kind => Value::enum_named(&ERROR_KIND, kind, Vec::new()),
        BuiltinId::RawOsError => Some(match code {
            Some(n) => Value::some(Value::Int(i64::from(*n))),
            None => Value::none(),
        }),
        _ => None,
    }
}

/// A task can end early only by panicking, so cancellation is always false.
pub(super) fn joinerr_method(handle: &Handle, method: &MethodName) -> Option<Value> {
    let h = handle.lock();
    let Native::JoinErr { is_panic, .. } = &*h else {
        return None;
    };
    match method.id {
        BuiltinId::IsPanic => Some(Value::Bool(*is_panic)),
        BuiltinId::IsCancelled => Some(Value::Bool(false)),
        _ => None,
    }
}

fn reader_native_method(
    handle: &Handle,
    method: &MethodName,
    args: &mut [Value],
) -> Result<Option<Value>> {
    match method.id {
        BuiltinId::ReadLine => {
            let mut h = handle.lock();
            let Some(r) = h.as_buf_read() else {
                bail!("read_line on non-reader {}", h.type_name());
            };
            let mut buf = String::new();
            let read = r.read_line(&mut buf);
            drop(h);
            return Ok(Some(io_err(read, |n| {
                if let Some(t) = args.first_mut() {
                    append_string(t, &buf);
                }
                Value::Int(int_len(n))
            })));
        }
        BuiltinId::ReadToString => {
            let mut h = handle.lock();
            let Some(r) = h.as_read() else {
                bail!("read_to_string on non-reader {}", h.type_name());
            };
            let mut buf = String::new();
            let read = r.read_to_string(&mut buf);
            drop(h);
            return Ok(Some(io_err(read, |n| {
                if let Some(t) = args.first_mut() {
                    append_string(t, &buf);
                }
                Value::Int(int_len(n))
            })));
        }
        BuiltinId::Read => {
            let mut h = handle.lock();
            let Some(r) = h.as_read() else {
                bail!("read on non-reader {}", h.type_name());
            };
            // the buffer arrives as a shared Vec, so copy back into it
            let len = match args.first() {
                Some(Value::Vec(v)) => v.lock().len(),
                _ => 0,
            };
            let mut buf = vec![0u8; len];
            let read = r.read(&mut buf);
            drop(h);
            return Ok(Some(io_err(read, |n| {
                if let Some(Value::Vec(v)) = args.first() {
                    let mut items = v.lock();
                    for (i, byte) in buf.iter().take(n).enumerate() {
                        items[i] = Value::Int(i64::from(*byte));
                    }
                }
                Value::Int(int_len(n))
            })));
        }
        BuiltinId::ReadToEnd => {
            let mut h = handle.lock();
            let Some(r) = h.as_read() else {
                bail!("read_to_end on non-reader {}", h.type_name());
            };
            let mut buf = Vec::new();
            let read = r.read_to_end(&mut buf);
            drop(h);
            return Ok(Some(io_err(read, |n| {
                if let Some(t) = args.first() {
                    append_bytes(t, &buf);
                }
                Value::Int(int_len(n))
            })));
        }
        // the delimiter is kept in the buffer like the real method, so a caller can tell a final
        // unterminated line
        BuiltinId::ReadUntil => {
            let delim = byte_arg(args.first(), "read_until")?;
            let mut h = handle.lock();
            let Some(r) = h.as_buf_read() else {
                bail!("read_until on non-reader {}", h.type_name());
            };
            let mut buf = Vec::new();
            let read = r.read_until(delim, &mut buf);
            drop(h);
            return Ok(Some(io_err(read, |n| {
                if let Some(t) = args.get(1) {
                    append_bytes(t, &buf);
                }
                Value::Int(int_len(n))
            })));
        }
        BuiltinId::Lines | BuiltinId::Next | BuiltinId::Collect => {
            return Ok(lines_native_method(handle, method));
        }
        _ => {}
    }
    Ok(None)
}

/// `lines()` moves the reader out, `next` and `collect` walk the iterator.
fn lines_native_method(handle: &Handle, method: &MethodName) -> Option<Value> {
    match method.id {
        BuiltinId::Lines => {
            // the original handle is left empty
            let taken = std::mem::replace(&mut *handle.lock(), Native::Taken);
            let iter: super::native::LineIter = match taken {
                Native::File(r) => {
                    use std::io::BufRead;
                    Box::new(r.lines())
                }
                Native::Reader(r) => {
                    use std::io::BufRead;
                    Box::new(r.lines())
                }
                other => {
                    *handle.lock() = other;
                    return None;
                }
            };
            Some(Native::Lines(iter).wrap())
        }
        BuiltinId::Next => {
            if matches!(&*handle.lock(), Native::Lines(_)) {
                return Some(match lines_next(handle) {
                    Some(v) => Value::some(v),
                    None => Value::none(),
                });
            }
            None
        }
        BuiltinId::Collect => {
            if matches!(&*handle.lock(), Native::Lines(_)) {
                return Some(Value::vec(drain_lines(handle)));
            }
            None
        }
        _ => None,
    }
}

fn writer_native_method(handle: &Handle, method: &MethodName, args: &mut [Value]) -> Option<Value> {
    match method.id {
        // the formatter buffer of a user `fmt` impl, the result is `fmt::Result`
        BuiltinId::WriteAll
        | BuiltinId::Write
        | BuiltinId::WriteStr
        | BuiltinId::WriteFmt
        | BuiltinId::Pad
            if matches!(&*handle.lock(), Native::Fmt { .. }) =>
        {
            let text = args.first().map(Value::display).unwrap_or_default();
            let mut h = handle.lock();
            if let Native::Fmt {
                text: buffer,
                padded,
            } = &mut *h
            {
                buffer.push_str(&text);
                *padded |= method.id == BuiltinId::Pad;
            }
            return Some(Value::ok(Value::Unit));
        }
        BuiltinId::WriteAll | BuiltinId::Write => {
            let bytes = value_to_bytes(args.first());
            let mut h = handle.lock();
            if !matches!(
                &*h,
                Native::File(_) | Native::Writer(_) | Native::ChildStdin(_) | Native::Stream(_)
            ) {
                return None;
            }
            let n = bytes.len();
            let r = write_bytes(&mut h, &bytes);
            let is_write = method.id == BuiltinId::Write;
            return Some(io_err(r, |()| {
                if is_write {
                    Value::Int(int_len(n))
                } else {
                    Value::Unit
                }
            }));
        }
        BuiltinId::Flush => {
            let mut h = handle.lock();
            let r = flush_writer(&mut h);
            return Some(io_err(r, |()| Value::Unit));
        }
        _ => {}
    }
    None
}

fn file_native_method(
    handle: &Handle,
    method: &MethodName,
    args: &mut [Value],
) -> Result<Option<Value>> {
    match method.id {
        BuiltinId::Seek => {
            let pos = seek_from(args.first());
            let mut h = handle.lock();
            if let Native::File(r) = &mut *h {
                return Ok(Some(io_err(r.seek(pos), |n| {
                    Value::Int(i64::try_from(n).unwrap_or(i64::MAX))
                })));
            }
            bail!("seek on non-file {}", h.type_name());
        }
        BuiltinId::SyncAll | BuiltinId::SyncData => {
            let mut h = handle.lock();
            if let Native::File(r) = &mut *h {
                return Ok(Some(io_err(r.get_ref().sync_all(), |()| Value::Unit)));
            }
            bail!("sync on non-file {}", h.type_name());
        }
        BuiltinId::SetLen => {
            let n = as_int(args.first())
                .and_then(|n| u64::try_from(n).ok())
                .unwrap_or(0);
            let mut h = handle.lock();
            if let Native::File(r) = &mut *h {
                return Ok(Some(io_err(r.get_ref().set_len(n), |()| Value::Unit)));
            }
            bail!("set_len on non-file {}", h.type_name());
        }
        BuiltinId::SetModified => {
            let time = match args.first() {
                Some(Value::Native(other)) => match &*other.lock() {
                    Native::SystemTime(t) => *t,
                    o => bail!("set_modified needs a SystemTime, got {}", o.type_name()),
                },
                _ => bail!("set_modified needs a SystemTime argument"),
            };
            let h = handle.lock();
            if let Native::File(r) = &*h {
                return Ok(Some(io_err(r.get_ref().set_modified(time), |()| {
                    Value::Unit
                })));
            }
            bail!("set_modified on non-file {}", h.type_name());
        }
        BuiltinId::Metadata => {
            let h = handle.lock();
            if let Native::File(r) = &*h {
                return Ok(Some(io_err(r.get_ref().metadata(), |m| {
                    super::std_bridge::make_metadata(&m)
                })));
            }
            bail!("metadata on non-file {}", h.type_name());
        }
        _ => {}
    }
    Ok(None)
}

fn child_native_method(handle: &Handle, method: &MethodName) -> Result<Option<Value>> {
    match method.id {
        BuiltinId::Wait => {
            let mut h = handle.lock();
            if let Native::Child(c) = &mut *h {
                return Ok(Some(io_err(c.wait(), |s| {
                    super::process::make_exit_status(s)
                })));
            }
            bail!("wait on non-child {}", h.type_name());
        }
        BuiltinId::TryWait => {
            let mut h = handle.lock();
            if let Native::Child(c) = &mut *h {
                return Ok(Some(match c.try_wait() {
                    Ok(Some(s)) => Value::ok(Value::some(super::process::make_exit_status(s))),
                    Ok(None) => Value::ok(Value::none()),
                    Err(e) => Value::err(Value::str(e.to_string())),
                }));
            }
            bail!("try_wait on non-child {}", h.type_name());
        }
        BuiltinId::Kill => {
            let mut h = handle.lock();
            if let Native::Child(c) = &mut *h {
                return Ok(Some(io_err(c.kill(), |()| Value::Unit)));
            }
            bail!("kill on non-child {}", h.type_name());
        }
        BuiltinId::Id => {
            let h = handle.lock();
            if let Native::Child(c) = &*h {
                return Ok(Some(Value::Int(i64::from(c.id()))));
            }
        }
        BuiltinId::WaitWithOutput => {
            if !matches!(&*handle.lock(), Native::Child(_)) {
                return Ok(None);
            }
            let taken = std::mem::replace(&mut *handle.lock(), Native::Taken);
            if let Native::Child(c) = taken {
                return Ok(Some(match c.wait_with_output() {
                    Ok(o) => Value::ok(super::process::make_output(o)),
                    Err(e) => Value::err(Value::str(e.to_string())),
                }));
            }
            bail!("wait_with_output on non-child");
        }
        _ => {}
    }
    Ok(None)
}

fn net_native_method(handle: &Handle, method: &MethodName) -> Result<Option<Value>> {
    match method.id {
        BuiltinId::Accept => {
            let h = handle.lock();
            if let Native::Listener(l) = &*h {
                return Ok(Some(match l.accept() {
                    Ok((stream, addr)) => Value::ok(Value::tuple(vec![
                        Native::Stream(stream).wrap(),
                        Value::str(addr.to_string()),
                    ])),
                    Err(e) => Value::err(Value::str(e.to_string())),
                }));
            }
            bail!("accept on non-listener {}", h.type_name());
        }
        BuiltinId::Incoming => {
            bail!("incoming() is not supported; loop with listener.accept() instead");
        }
        BuiltinId::LocalAddr => {
            let h = handle.lock();
            let addr = match &*h {
                Native::Listener(l) => l.local_addr(),
                Native::Stream(s) => s.local_addr(),
                Native::Udp(s) => s.local_addr(),
                _ => bail!("local_addr on {}", h.type_name()),
            };
            return Ok(Some(io_err(addr, |a| Value::str(a.to_string()))));
        }
        BuiltinId::PeerAddr => {
            let h = handle.lock();
            if let Native::Stream(s) = &*h {
                return Ok(Some(io_err(s.peer_addr(), |a| Value::str(a.to_string()))));
            }
            bail!("peer_addr on {}", h.type_name());
        }
        BuiltinId::Shutdown => {
            let h = handle.lock();
            if let Native::Stream(s) = &*h {
                return Ok(Some(io_err(s.shutdown(std::net::Shutdown::Both), |()| {
                    Value::Unit
                })));
            }
            bail!("shutdown on {}", h.type_name());
        }
        BuiltinId::TryClone => {
            let h = handle.lock();
            match &*h {
                Native::Stream(s) => {
                    return Ok(Some(io_err(s.try_clone(), |s| Native::Stream(s).wrap())));
                }
                Native::Udp(s) => {
                    return Ok(Some(io_err(s.try_clone(), |s| Native::Udp(s).wrap())));
                }
                _ => bail!("try_clone on {}", h.type_name()),
            }
        }
        _ => {}
    }
    Ok(None)
}

fn udp_native_method(
    handle: &Handle,
    method: &MethodName,
    args: &mut [Value],
) -> Result<Option<Value>> {
    match method.id {
        BuiltinId::SetBroadcast => {
            let on = matches!(args.first(), Some(Value::Bool(true)));
            let h = handle.lock();
            if let Native::Udp(s) = &*h {
                return Ok(Some(io_err(s.set_broadcast(on), |()| Value::Unit)));
            }
            bail!("set_broadcast on {}", h.type_name());
        }
        BuiltinId::SendTo => {
            let bytes = value_to_bytes(args.first());
            let addr = args.get(1).map(Value::display).unwrap_or_default();
            let h = handle.lock();
            if let Native::Udp(s) = &*h {
                return Ok(Some(io_err(s.send_to(&bytes, addr), |n| {
                    Value::Int(int_len(n))
                })));
            }
            bail!("send_to on {}", h.type_name());
        }
        BuiltinId::Send => {
            let bytes = value_to_bytes(args.first());
            let h = handle.lock();
            if let Native::Udp(s) = &*h {
                return Ok(Some(io_err(s.send(&bytes), |n| Value::Int(int_len(n)))));
            }
            bail!("send on {}", h.type_name());
        }
        BuiltinId::Connect => {
            let addr = args.first().map(Value::display).unwrap_or_default();
            let h = handle.lock();
            if let Native::Udp(s) = &*h {
                return Ok(Some(io_err(s.connect(addr), |()| Value::Unit)));
            }
            bail!("connect on {}", h.type_name());
        }
        _ => {}
    }
    Ok(None)
}

fn time_native_method(
    handle: &Handle,
    method: &MethodName,
    args: &mut [Value],
) -> Result<Option<Value>> {
    match method.id {
        BuiltinId::Elapsed => {
            let h = handle.lock();
            match &*h {
                Native::Instant(t) => {
                    return Ok(Some(super::std_bridge::make_duration(t.elapsed())));
                }
                Native::SystemTime(t) => {
                    return Ok(Some(match t.elapsed() {
                        Ok(d) => Value::ok(super::std_bridge::make_duration(d)),
                        Err(e) => Value::err(Value::str(e.to_string())),
                    }));
                }
                _ => bail!("elapsed on {}", h.type_name()),
            }
        }
        BuiltinId::DurationSince => {
            let h = handle.lock();
            match (&*h, args.first()) {
                (Native::Instant(t), Some(Value::Native(other))) => {
                    if let Native::Instant(o) = &*other.lock() {
                        return Ok(Some(super::std_bridge::make_duration(t.duration_since(*o))));
                    }
                }
                (Native::SystemTime(t), Some(Value::Native(other))) => {
                    if let Native::SystemTime(o) = &*other.lock() {
                        return Ok(Some(match t.duration_since(*o) {
                            Ok(d) => Value::ok(super::std_bridge::make_duration(d)),
                            Err(e) => Value::err(Value::str(e.to_string())),
                        }));
                    }
                }
                _ => {}
            }
            bail!("duration_since arguments mismatch");
        }
        _ => {}
    }
    Ok(None)
}

fn temp_native_method(handle: &Handle, method: &MethodName) -> Result<Option<Value>> {
    match method.id {
        BuiltinId::Path => {
            let h = handle.lock();
            match &*h {
                Native::TempDir(d) => {
                    return Ok(Some(super::std_bridge::make_path(
                        d.path().display().to_string(),
                    )));
                }
                Native::NamedTempFile(f) => {
                    return Ok(Some(super::std_bridge::make_path(
                        f.path().display().to_string(),
                    )));
                }
                _ => {}
            }
        }
        BuiltinId::Close => {
            if !matches!(&*handle.lock(), Native::TempDir(_)) {
                return Ok(None);
            }
            let taken = std::mem::replace(&mut *handle.lock(), Native::Taken);
            if let Native::TempDir(d) = taken {
                return Ok(Some(io_err(d.close(), |()| Value::Unit)));
            }
            bail!("close on non-tempdir");
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

pub(super) fn value_to_bytes(v: Option<&Value>) -> Vec<u8> {
    match v {
        Some(Value::Str(s)) => s.as_bytes().to_vec(),
        Some(Value::Vec(items)) => items
            .lock()
            .iter()
            .filter_map(|x| match x {
                Value::Int(i) => u8::try_from(*i).ok(),
                Value::IntW(v, w) => u8::try_from(w.decode(*v)).ok(),
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
    // `SeekFrom::Start(n)` is an enum value carrying the offset
    if let Some(Value::Enum { def, variant, data }) = v {
        let n = data.lock().first().and_then(|x| match x {
            Value::Int(i) => Some(*i),
            _ => None,
        });
        match (&**def.variant_name(*variant), n) {
            ("Start", Some(n)) => return SeekFrom::Start(u64::try_from(n).unwrap_or_default()),
            ("End", Some(n)) => return SeekFrom::End(n),
            ("Current", Some(n)) => return SeekFrom::Current(n),
            _ => {}
        }
    }
    SeekFrom::Current(0)
}
