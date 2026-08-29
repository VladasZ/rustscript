//! The in place `collect` of std. A `vec.into_iter()` chain of `map`, `skip` and `cloned` that
//! lands in a `Vec` of a compatible layout is filled by index, so std reads the size first and a
//! `skip` past the end never runs a closure. Every other shape drains lazily, which visits the
//! same elements in the same order, so only the size 0 case needs modelling.

use super::{Handle, IteratorState};
use crate::interpreter::bytecode::ScalarTy;
use crate::interpreter::native::Native;
use crate::interpreter::value::Value;

/// Size and alignment in bytes.
type Layout = (usize, usize);

pub(super) fn collects_nothing(iterator: &Handle, target: Option<&ScalarTy>) -> bool {
    let Some(ScalarTy::List(dest)) = target else {
        return false;
    };
    let Some((size, source)) = probe(iterator) else {
        return false;
    };
    if size != 0 {
        return false;
    }
    match source {
        // an empty source runs nothing either way
        None => true,
        Some(source) => layout_of_scalar(dest).is_some_and(|dest| fits(source, dest)),
    }
}

/// `in_place_collectible` of std, same alignment and the source element no smaller.
fn fits(source: Layout, dest: Layout) -> bool {
    source.0 > 0 && dest.0 > 0 && source.1 == dest.1 && source.0 >= dest.0
}

/// The size the chain reports and the layout of the source element, `None` for the first
/// element of an empty source.
fn probe(iterator: &Handle) -> Option<(usize, Option<Layout>)> {
    let guard = iterator.lock();
    let Native::Iterator(state) = &*guard else {
        return None;
    };
    match state {
        IteratorState::Values {
            values,
            index,
            owned: true,
            back,
        } => {
            let items = values.lock();
            Some(rest_of(
                items.get(*index..items.len().saturating_sub(*back))?,
            ))
        }
        IteratorState::Owned {
            values,
            index,
            vec: true,
        } => Some(rest_of(values.get(*index..)?)),
        // `Rev` is not random access in std, a `skip` after it drains lazily
        IteratorState::Map { source, .. } | IteratorState::Cloned { source } => probe(source),
        IteratorState::Skip { source, remaining } => {
            let (size, layout) = probe(source)?;
            Some((size.saturating_sub(*remaining), layout))
        }
        _ => None,
    }
}

fn layout_of_scalar(ty: &ScalarTy) -> Option<Layout> {
    Some(match ty {
        ScalarTy::Int(width) => int_layout(width.bits()),
        ScalarTy::F32 | ScalarTy::Char => (4, 4),
        ScalarTy::F64 => (8, 8),
        ScalarTy::Bool => (1, 1),
        ScalarTy::Str | ScalarTy::List(_) => (24, 8),
        ScalarTy::Opt(_) | ScalarTy::Map(_) | ScalarTy::Set(_) | ScalarTy::Other => return None,
    })
}

fn layout_of_value(value: &Value) -> Option<Layout> {
    Some(match value {
        Value::Unit => (0, 1),
        Value::Bool(_) => (1, 1),
        Value::Int(_) | Value::Float(_) => (8, 8),
        Value::IntW(_, width) | Value::Big(_, width) => int_layout(width.bits()),
        Value::F32(_) | Value::Char(_) => (4, 4),
        Value::Str(_) | Value::Vec(_) => (24, 8),
        Value::Tuple(items) => {
            let items = items.lock();
            let mut size = 0;
            let mut align = 1;
            for item in items.iter() {
                let (item_size, item_align) = layout_of_value(item)?;
                size += item_size;
                align = align.max(item_align);
            }
            (size.next_multiple_of(align), align)
        }
        _ => return None,
    })
}

fn int_layout(bits: u32) -> Layout {
    let bytes = bits as usize / 8;
    (bytes, bytes)
}

fn rest_of(rest: &[Value]) -> (usize, Option<Layout>) {
    (rest.len(), rest.first().and_then(layout_of_value))
}
