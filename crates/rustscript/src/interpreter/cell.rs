//! `Rc`, `Arc`, `RefCell`, `Cell` and `Mutex` as real shared cells. Cloning
//! shares the slot, everything else in the value model copies on mutation.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use parking_lot::Mutex;

use super::bridge::arg;
use super::bytecode::BuiltinId;
use super::value::{CellKind, Value, ValueRef};

pub(super) fn make_cell(kind: CellKind, inner: Value) -> Value {
    Value::Cell(kind, Arc::new(Mutex::new(inner)))
}

/// Anything else reads through to the content in `eval_method`.
pub(super) fn cell_method(
    kind: CellKind,
    slot: &Arc<Mutex<Value>>,
    name: BuiltinId,
    args: &mut [Value],
) -> Result<Option<Value>> {
    // An interior method on `Rc<RefCell<..>>` auto derefs to the inner cell.
    if kind.is_shared_pointer() && interior_method(name) {
        let inner = slot.lock().clone();
        let Value::Cell(inner_kind, inner_slot) = inner else {
            bail!("no method `{}` on {}", name.name(), kind_name(kind));
        };
        return cell_method(inner_kind, &inner_slot, name, args);
    }
    Ok(Some(match name {
        BuiltinId::Clone => Value::Cell(kind, slot.clone()),
        BuiltinId::Borrow => {
            require(kind, CellKind::RefCell, name)?;
            slot.lock().clone()
        }
        BuiltinId::BorrowMut => {
            require(kind, CellKind::RefCell, name)?;
            Value::Ref(Arc::new(ValueRef::cell_slot(slot.clone())))
        }
        BuiltinId::Lock | BuiltinId::TryLock | BuiltinId::BlockingLock => {
            // The tokio mutex hands its guard out directly, only `try_lock`
            // wraps a `Result`. The std mutex wraps either way.
            if kind == CellKind::TokioMutex {
                let guard = Value::Ref(Arc::new(ValueRef::cell_slot(slot.clone())));
                if name == BuiltinId::TryLock {
                    Value::ok(guard)
                } else {
                    guard
                }
            } else {
                require(kind, CellKind::Mutex, name)?;
                if name == BuiltinId::BlockingLock {
                    bail!("no method `blocking_lock` on std Mutex");
                }
                Value::ok(Value::Ref(Arc::new(ValueRef::cell_slot(slot.clone()))))
            }
        }
        BuiltinId::Get if kind == CellKind::Cell => slot.lock().clone(),
        BuiltinId::Set => {
            require_interior(kind, name)?;
            let new = arg(args, 0)?;
            *slot.lock() = new;
            Value::Unit
        }
        BuiltinId::Replace => {
            require_interior(kind, name)?;
            let new = arg(args, 0)?;
            let mut guard = slot.lock();
            let old = guard.clone();
            *guard = new;
            old
        }
        BuiltinId::Take => {
            require_interior(kind, name)?;
            let mut guard = slot.lock();
            let old = guard.clone();
            *guard = old.default_like();
            old
        }
        BuiltinId::GetMut => {
            require_interior(kind, name)?;
            Value::Ref(Arc::new(ValueRef::cell_slot(slot.clone())))
        }
        BuiltinId::IntoInner => slot.lock().clone(),
        _ => return Ok(None),
    }))
}

/// An interior mutability method that auto derefs through `Rc` and `Arc`.
fn interior_method(name: BuiltinId) -> bool {
    matches!(
        name,
        BuiltinId::Borrow
            | BuiltinId::BorrowMut
            | BuiltinId::Lock
            | BuiltinId::TryLock
            | BuiltinId::BlockingLock
            | BuiltinId::Get
            | BuiltinId::Set
            | BuiltinId::Replace
            | BuiltinId::Take
            | BuiltinId::GetMut
    )
}

fn kind_name(kind: CellKind) -> &'static str {
    match kind {
        CellKind::Rc => "Rc",
        CellKind::Arc => "Arc",
        CellKind::RefCell => "RefCell",
        CellKind::Cell => "Cell",
        CellKind::Mutex | CellKind::TokioMutex => "Mutex",
    }
}

fn require(kind: CellKind, wanted: CellKind, name: BuiltinId) -> Result<()> {
    if kind == wanted {
        Ok(())
    } else {
        Err(anyhow!(
            "no method `{}` on {}",
            name.name(),
            kind_name(kind)
        ))
    }
}

fn require_interior(kind: CellKind, name: BuiltinId) -> Result<()> {
    if kind.is_shared_pointer() {
        Err(anyhow!(
            "no method `{}` on {}",
            name.name(),
            kind_name(kind)
        ))
    } else {
        Ok(())
    }
}
