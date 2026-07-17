//! A focused PDF bridge backed by lopdf. It exposes only the operations the
//! fix_export_pdf skill script needs, loading a document, reading and replacing
//! a page's decoded content stream, and saving. The byte level editing stays in
//! the script, this bridge is only the PDF plumbing lopdf provides.

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use super::native::Native;
use super::value::Value;

pub(super) fn load(path: &str) -> Value {
    match Document::load(path) {
        Ok(doc) => Value::ok(Native::Pdf(Box::new(doc)).wrap()),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

fn page_ids(doc: &Document) -> Vec<ObjectId> {
    doc.get_pages().into_values().collect()
}

fn page_at(doc: &Document, index: i64) -> Option<ObjectId> {
    usize::try_from(index)
        .ok()
        .and_then(|i| page_ids(doc).get(i).copied())
}

pub(super) fn page_count(doc: &Document) -> Value {
    Value::Int(i64::try_from(page_ids(doc).len()).unwrap_or(0))
}

pub(super) fn page_content(doc: &Document, index: i64) -> Value {
    let Some(id) = page_at(doc, index) else {
        return Value::err(Value::str(format!("page index {index} out of range")));
    };
    let bytes = doc.get_page_content(id);
    Value::ok(Value::vec(
        bytes.iter().map(|b| Value::Int(i64::from(*b))).collect(),
    ))
}

pub(super) fn set_page_content(doc: &mut Document, index: i64, bytes: Vec<u8>) -> Value {
    let Some(id) = page_at(doc, index) else {
        return Value::err(Value::str(format!("page index {index} out of range")));
    };
    let stream_id = doc.add_object(Stream::new(Dictionary::new(), bytes));
    match doc.get_object_mut(id).and_then(Object::as_dict_mut) {
        Ok(dict) => {
            dict.set("Contents", Object::Reference(stream_id));
            Value::ok(Value::Unit)
        }
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

pub(super) fn save(doc: &mut Document, path: &str) -> Value {
    match doc.save(path) {
        Ok(_) => Value::ok(Value::Unit),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}
