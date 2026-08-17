//! `Rc`, `Arc`, `RefCell`, `Cell`, and `Mutex` as real shared cells.
//!
//! These are the types real Rust uses when sharing is the point, so their
//! sharing stays observable: cloning a cell shares its slot, and a write
//! through one handle shows through every handle. Everything else in the
//! value model copies on mutation instead.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use parking_lot::Mutex;

use super::value::{CellKind, Value, ValueRef};

pub(super) fn make_cell(kind: CellKind, inner: Value) -> Value {
    Value::Cell(kind, Arc::new(Mutex::new(inner)))
}

/// The methods that belong to the wrapper itself. Anything else reads
/// through to the content, `eval_method` handles that fallthrough.
pub(super) fn cell_method(
    kind: CellKind,
    slot: &Arc<Mutex<Value>>,
    name: &str,
    args: &mut [Value],
) -> Result<Option<Value>> {
    // An interior method on an `Rc<RefCell<..>>` derefs the shared pointer
    // and lands on the inner cell, the way real auto-deref does.
    if kind.is_shared_pointer() && interior_method(name) {
        let inner = slot.lock().clone();
        let Value::Cell(inner_kind, inner_slot) = inner else {
            bail!("no method `{name}` on {}", kind_name(kind));
        };
        return cell_method(inner_kind, &inner_slot, name, args);
    }
    Ok(Some(match name {
        "clone" => Value::Cell(kind, slot.clone()),
        "borrow" => {
            require(kind, CellKind::RefCell, name)?;
            slot.lock().clone()
        }
        "borrow_mut" => {
            require(kind, CellKind::RefCell, name)?;
            Value::Ref(Arc::new(ValueRef::cell_slot(slot.clone())))
        }
        "lock" | "try_lock" => {
            require(kind, CellKind::Mutex, name)?;
            Value::ok(Value::Ref(Arc::new(ValueRef::cell_slot(slot.clone()))))
        }
        "get" if kind == CellKind::Cell => slot.lock().clone(),
        "set" => {
            require_interior(kind, name)?;
            let new = args.first().cloned().unwrap_or(Value::Unit);
            *slot.lock() = new;
            Value::Unit
        }
        "replace" => {
            require_interior(kind, name)?;
            let new = args.first().cloned().unwrap_or(Value::Unit);
            let mut guard = slot.lock();
            let old = guard.clone();
            *guard = new;
            old
        }
        "take" => {
            require_interior(kind, name)?;
            let mut guard = slot.lock();
            let old = guard.clone();
            *guard = old.default_like();
            old
        }
        "get_mut" => {
            require_interior(kind, name)?;
            Value::Ref(Arc::new(ValueRef::cell_slot(slot.clone())))
        }
        "into_inner" => slot.lock().clone(),
        _ => return Ok(None),
    }))
}

/// Whether the name is an interior-mutability method that auto-derefs
/// through Rc and Arc to the cell inside.
fn interior_method(name: &str) -> bool {
    matches!(
        name,
        "borrow"
            | "borrow_mut"
            | "lock"
            | "try_lock"
            | "get"
            | "set"
            | "replace"
            | "take"
            | "get_mut"
    )
}

fn kind_name(kind: CellKind) -> &'static str {
    match kind {
        CellKind::Rc => "Rc",
        CellKind::Arc => "Arc",
        CellKind::RefCell => "RefCell",
        CellKind::Cell => "Cell",
        CellKind::Mutex => "Mutex",
    }
}

fn require(kind: CellKind, wanted: CellKind, name: &str) -> Result<()> {
    if kind == wanted {
        Ok(())
    } else {
        Err(anyhow!("no method `{name}` on {}", kind_name(kind)))
    }
}

fn require_interior(kind: CellKind, name: &str) -> Result<()> {
    if kind.is_shared_pointer() {
        Err(anyhow!("no method `{name}` on {}", kind_name(kind)))
    } else {
        Ok(())
    }
}
