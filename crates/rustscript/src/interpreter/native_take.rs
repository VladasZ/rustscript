//! `Read::take` on a reader handle. Every read wraps the source in the real `io::Take`, so the
//! limit counts and runs out exactly like std.

use std::io::{BufRead, Read};

use anyhow::{Result, bail};

use super::bytecode::{BuiltinId, MethodName};
use super::native::Native;
use super::native_methods::Handle;
use super::numeric::IntWidth;
use super::value::Value;

/// The handle a `Take` reads through. A std stream keeps its handle in its `inner` field.
fn source_handle(source: &Value) -> Option<Handle> {
    match source {
        Value::Native(h) => Some(h.clone()),
        Value::Struct(st) if &**st.name() == "StdStream" => match st.get("inner") {
            Some(Value::Native(h)) => Some(h),
            _ => None,
        },
        _ => None,
    }
}

/// Runs `f` on the reader behind `handle`, `None` when it is no reader. A `Take` hands `f` the
/// real `io::Take` over its source and keeps the limit it leaves, so takes nest like std.
pub(super) fn with_buf_read<R>(
    handle: &Handle,
    f: &mut dyn FnMut(&mut dyn BufRead) -> R,
) -> Option<R> {
    let mut h = handle.lock();
    if let Native::Take { source, limit } = &mut *h {
        let inner = source_handle(source)?;
        let start = *limit;
        let mut left = start;
        let out = with_buf_read(&inner, &mut |r| {
            let mut take = Read::take(r, start);
            let out = f(&mut take);
            left = take.limit();
            out
        });
        *limit = left;
        return out;
    }
    h.as_buf_read().map(f)
}

/// The next line of a `Take` for `lines()`, with the line end cut off like `BufRead::lines`.
pub(super) fn take_line(handle: &Handle) -> Option<std::io::Result<String>> {
    let mut line = String::new();
    match with_buf_read(handle, &mut |r| r.read_line(&mut line))? {
        Ok(0) => None,
        Ok(_) => {
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            Some(Ok(line))
        }
        Err(e) => Some(Err(e)),
    }
}

fn limit_arg(arg: Option<&Value>, method: &str) -> Result<u64> {
    let parts = arg.and_then(Value::int_parts);
    match parts.and_then(|(n, _)| u64::try_from(n).ok()) {
        Some(n) => Ok(n),
        None => bail!("{method} needs a u64 limit"),
    }
}

/// A new `Take` over `source`, the receiver value itself, so `into_inner` gives it back.
pub(super) fn make_take(source: Value, args: &[Value]) -> Result<Value> {
    let limit = limit_arg(args.first(), "take")?;
    Ok(Native::Take { source, limit }.wrap())
}

/// `take` and `by_ref` on a reader, and the own methods of a `Take`. The read methods are the
/// reader ones. A handle is shared, so `by_ref` is the handle itself.
pub(super) fn take_native_method(
    handle: &Handle,
    method: &MethodName,
    args: &mut [Value],
) -> Result<Option<Value>> {
    let mut h = handle.lock();
    let is_reader = matches!(&*h, Native::Take { .. }) || h.as_buf_read().is_some();
    if is_reader && method.id == BuiltinId::ByRef {
        return Ok(Some(Value::Native(handle.clone())));
    }
    if is_reader && method.id == BuiltinId::Take && args.len() == 1 {
        drop(h);
        return make_take(Value::Native(handle.clone()), args).map(Some);
    }
    let Native::Take { source, limit } = &mut *h else {
        return Ok(None);
    };
    Ok(match method.id {
        BuiltinId::Limit => Some(Value::int_of_width(i128::from(*limit), IntWidth::U64)),
        BuiltinId::SetLimit => {
            *limit = limit_arg(args.first(), "set_limit")?;
            Some(Value::Unit)
        }
        BuiltinId::IntoInner | BuiltinId::GetRef | BuiltinId::GetMut => Some(source.clone()),
        _ => None,
    })
}
