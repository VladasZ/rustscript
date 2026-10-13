//! `Vec` methods that cut the vec apart, `drain`, `split_off`, `chunks_exact`, `rchunks` and
//! the `first_chunk` family, and `is_sorted`.

use std::cmp::Ordering;
use std::sync::Arc;

use anyhow::{Result, bail};
use parking_lot::Mutex;

use super::bytecode::BuiltinId;
use super::iterator;
use super::ops::partial_compare;
use super::value::{List, Value};
use super::vecmap::int_arg;

/// `None` for any other method.
pub(super) fn vec_edit(v: &List, id: BuiltinId, args: &[Value]) -> Result<Option<Value>> {
    Ok(Some(match id {
        // The drained items leave the vec at the call. std takes them out when the `Drain`
        // drops, which shows only through `mem::forget`.
        BuiltinId::Drain => {
            let mut items = v.lock();
            let Some(Value::Range {
                start,
                end,
                inclusive,
                ..
            }) = args.first()
            else {
                bail!("drain takes a range");
            };
            let len = items.len();
            // std checks the start against the length, then the end, then the order
            let start = usize::try_from(*start)?;
            let end = match (*end, *inclusive) {
                (i64::MAX, _) => len,
                (end, true) => usize::try_from(end)? + 1,
                (end, false) => usize::try_from(end)?,
            };
            if start > len {
                bail!("range start index {start} out of range for slice of length {len}");
            }
            if end > len {
                bail!("range end index {end} out of range for slice of length {len}");
            }
            if start > end {
                bail!("slice index starts at {start} but ends at {end}");
            }
            iterator::owned_iterator(items.drain(start..end).collect())
        }
        BuiltinId::SplitOff => {
            let at = usize::try_from(int_arg(args, 0)?)?;
            let mut items = v.lock();
            if at > items.len() {
                bail!(
                    "`at` split index (is {at}) should be <= len (is {})",
                    items.len()
                );
            }
            Value::vec(items.split_off(at))
        }
        BuiltinId::ChunksExact | BuiltinId::Rchunks => {
            let size = usize::try_from(int_arg(args, 0)?)?;
            if size == 0 {
                bail!("chunk size must be non-zero");
            }
            let items = v.lock();
            let chunks: Vec<Value> = if id == BuiltinId::ChunksExact {
                items
                    .chunks_exact(size)
                    .map(|chunk| Value::vec(chunk.to_vec()))
                    .collect()
            } else {
                items
                    .rchunks(size)
                    .map(|chunk| Value::vec(chunk.to_vec()))
                    .collect()
            };
            iterator::value_iter(Arc::new(Mutex::new(chunks)))
        }
        BuiltinId::RotateLeft | BuiltinId::RotateRight => {
            let by = usize::try_from(int_arg(args, 0)?)?;
            let mut items = v.lock();
            if by > items.len() {
                let name = if id == BuiltinId::RotateLeft {
                    "mid"
                } else {
                    "k"
                };
                bail!("assertion failed: {name} <= self.len()");
            }
            if id == BuiltinId::RotateLeft {
                items.rotate_left(by);
            } else {
                items.rotate_right(by);
            }
            Value::Unit
        }
        BuiltinId::FirstChunk
        | BuiltinId::LastChunk
        | BuiltinId::SplitFirstChunk
        | BuiltinId::SplitLastChunk => chunk_edge(v, id, args)?,
        BuiltinId::IsSorted => {
            let items = v.lock();
            let mut sorted = true;
            for pair in items.windows(2) {
                if !matches!(
                    partial_compare(&pair[0], &pair[1])?,
                    Some(Ordering::Less | Ordering::Equal)
                ) {
                    sorted = false;
                    break;
                }
            }
            Value::Bool(sorted)
        }
        _ => return Ok(None),
    }))
}

/// `first_chunk::<N>()` and its 3 siblings. The `N` is part of the type, so the compiler hands
/// it over as the argument. `None` when the slice is shorter than `N`. The parts are copies like
/// every slice a call hands back.
fn chunk_edge(v: &List, id: BuiltinId, args: &[Value]) -> Result<Value> {
    let Some(len) = args.first().and_then(Value::int_parts) else {
        bail!("the length of the chunk must stand in a turbofish, like `first_chunk::<2>()`");
    };
    let len = usize::try_from(len.0)?;
    let items = v.lock();
    let front = matches!(id, BuiltinId::FirstChunk | BuiltinId::SplitFirstChunk);
    let mid = if front {
        (len <= items.len()).then_some(len)
    } else {
        items.len().checked_sub(len)
    };
    let Some(mid) = mid else {
        return Ok(Value::none());
    };
    let (head, tail) = items.split_at(mid);
    let (head, tail) = (Value::vec(head.to_vec()), Value::vec(tail.to_vec()));
    Ok(Value::some(match id {
        BuiltinId::FirstChunk => head,
        BuiltinId::LastChunk => tail,
        // the chunk comes first out of `split_first_chunk` and last out of `split_last_chunk`
        _ => Value::tuple(vec![head, tail]),
    }))
}
