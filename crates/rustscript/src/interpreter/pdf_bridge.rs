//! The lopdf bridge, the real `Document` API subset. An `ObjectId` is the `(u32, u16)` tuple
//! lopdf defines.

use anyhow::{Result, bail};
use lopdf::{Document, ObjectId};

use super::bytecode::{BuiltinId, MethodName};
use super::native::Native;
use super::numeric::IntWidth;
use super::std_bridge::as_i64;
use super::value::Value;

pub(super) fn load(path: &str) -> Value {
    match Document::load(path) {
        Ok(doc) => Value::ok(Native::Pdf(Box::new(doc)).wrap()),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

pub(super) fn document_method(
    doc: &mut Document,
    name: &MethodName,
    args: &[Value],
) -> Result<Option<Value>> {
    Ok(Some(match name.id {
        // page number to `ObjectId`, as a map of int to tuple
        BuiltinId::GetPages => {
            let mut map = crate::interpreter::value::MapStore::default();
            for (num, id) in doc.get_pages() {
                let key = Value::int_of_width(i128::from(num), IntWidth::U32)
                    .into_key()
                    .expect("an int is always a valid map key");
                map.insert(key, object_id_value(id));
            }
            Value::map_of(map)
        }
        BuiltinId::GetPageContent => {
            let id = object_id_arg(args, 0)?;
            let bytes = doc.get_page_content(id);
            Value::vec(bytes.into_iter().map(Value::byte).collect())
        }
        BuiltinId::ChangePageContent => {
            let id = object_id_arg(args, 0)?;
            let bytes = bytes_arg(args, 1);
            match doc.change_page_content(id, bytes) {
                Ok(()) => Value::ok(Value::Unit),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        // the real save returns the File, scripts drop it
        BuiltinId::Save => {
            let path = args.first().map(Value::display).unwrap_or_default();
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
        Value::int_of_width(i128::from(id.0), IntWidth::U32),
        Value::int_of_width(i128::from(id.1), IntWidth::U16),
    ])
}

fn object_id_arg(args: &[Value], i: usize) -> Result<ObjectId> {
    if let Some(Value::Tuple(items)) = args.get(i) {
        let items = items.lock();
        let part = |slot: usize| items.get(slot).and_then(Value::int_parts);
        if let (Some((a, _)), Some((b, _))) = (part(0), part(1)) {
            return Ok((u32::try_from(a)?, u16::try_from(b)?));
        }
    }
    bail!("expected a page ObjectId tuple like the ones get_pages returns");
}

fn bytes_arg(args: &[Value], i: usize) -> Vec<u8> {
    let Some(Value::Vec(items)) = args.get(i) else {
        return Vec::new();
    };
    items
        .lock()
        .iter()
        .filter_map(|v| as_i64(v).and_then(|n| u8::try_from(n).ok()))
        .collect()
}
