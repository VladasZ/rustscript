//! The lopdf bridge. Exposes the real `lopdf::Document` API subset instead of
//! an invented type, so a script using it is valid Rust that compiles with the
//! actual crate and passes `rust check`: `Document::load`, `get_pages`,
//! `get_page_content`, `change_page_content`, and `save`. An `ObjectId` is the
//! `(u32, u16)` tuple lopdf defines, carried here as a plain tuple value.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Result, bail};
use lopdf::{Document, ObjectId};

use super::native::Native;
use super::value::{Map, Value};

pub(super) fn load(path: &str) -> Value {
    match Document::load(path) {
        Ok(doc) => Value::ok(Native::Pdf(Box::new(doc)).wrap()),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

/// Methods on a loaded `Document`, mirroring lopdf's names and shapes.
pub(super) fn document_method(
    doc: &mut Document,
    name: &str,
    args: &[Value],
) -> Result<Option<Value>> {
    Ok(Some(match name {
        // BTreeMap of page number to page ObjectId, as a map of int to tuple.
        "get_pages" => {
            let mut map = Map::default();
            for (num, id) in doc.get_pages() {
                let key = Value::Int(i64::from(num))
                    .into_key()
                    .expect("an int is always a valid map key");
                map.insert(key, object_id_value(id));
            }
            Value::Map(Rc::new(RefCell::new(map)))
        }
        "get_page_content" => {
            let id = object_id_arg(args, 0)?;
            let bytes = doc.get_page_content(id);
            Value::vec(
                bytes
                    .into_iter()
                    .map(|b| Value::Int(i64::from(b)))
                    .collect(),
            )
        }
        "change_page_content" => {
            let id = object_id_arg(args, 0)?;
            let bytes = bytes_arg(args, 1);
            match doc.change_page_content(id, bytes) {
                Ok(()) => Value::ok(Value::Unit),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        // The real save returns the created File; scripts drop it, so Unit.
        "save" => {
            let path = args.first().map(|v| v.display()).unwrap_or_default();
            match doc.save(&path) {
                Ok(_) => Value::ok(Value::Unit),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        _ => return Ok(None),
    }))
}

fn object_id_value(id: ObjectId) -> Value {
    Value::tuple(vec![
        Value::Int(i64::from(id.0)),
        Value::Int(i64::from(id.1)),
    ])
}

/// An `ObjectId` argument, the `(u32, u16)` tuple `get_pages` handed out.
fn object_id_arg(args: &[Value], i: usize) -> Result<ObjectId> {
    if let Some(Value::Tuple(items)) = args.get(i) {
        let items = items.borrow();
        if let (Some(Value::Int(a)), Some(Value::Int(b))) = (items.first(), items.get(1)) {
            return Ok((*a as u32, *b as u16));
        }
    }
    bail!("expected a page ObjectId tuple like the ones get_pages returns");
}

/// A `Vec<u8>` argument, a list of byte-sized ints.
fn bytes_arg(args: &[Value], i: usize) -> Vec<u8> {
    let Some(Value::Vec(items)) = args.get(i) else {
        return Vec::new();
    };
    items
        .borrow()
        .iter()
        .filter_map(|v| match v {
            Value::Int(n) => Some(*n as u8),
            _ => None,
        })
        .collect()
}
