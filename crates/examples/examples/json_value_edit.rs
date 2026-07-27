#!/usr/bin/env rust

// Editing a parsed `serde_json::Value` in place. The mut accessors hand back
// the real container, not a copy, so an insert or a push through one lands in
// the value it was taken from. The variant constructors build a json value out
// of its payload.
//
// Whole objects are printed only where the keys go in alphabetical order, since
// serde_json sorts an object's keys and this prints the same either way.

use serde_json::Value;

// The shape every real caller has: take an args object and stamp one more field
// into it before the call goes out.
fn tag(value: &mut Value, console: &str) {
    if let Some(object) = value.as_object_mut() {
        object.insert("console".to_string(), Value::String(console.to_string()));
    }
}

fn main() {
    let mut request: Value = serde_json::from_str(r#"{"cursor":null,"timeout_ms":15000}"#).unwrap();
    tag(&mut request, "m48");
    println!(
        "console {}",
        request.get("console").unwrap().as_str().unwrap()
    );
    println!(
        "timeout {}",
        request.get("timeout_ms").unwrap().as_i64().unwrap()
    );

    // A second insert through a fresh handle, proving the first one really
    // changed the value rather than a copy of it.
    if let Some(object) = request.as_object_mut() {
        object.insert("newline".to_string(), Value::Bool(true));
        object.insert("retries".to_string(), Value::Number(2.into()));
    }
    println!("keys {}", request.as_object().unwrap().len());
    println!(
        "newline {}",
        request.get("newline").unwrap().as_bool().unwrap()
    );
    println!(
        "retries {}",
        request.get("retries").unwrap().as_i64().unwrap()
    );

    // The array side of the same pair.
    let mut list: Value = serde_json::from_str(r#"["a","b"]"#).unwrap();
    if let Some(items) = list.as_array_mut() {
        items.push(Value::String("c".to_string()));
    }
    println!("list {}", serde_json::to_string(&list).unwrap());
    println!("len {}", list.as_array().unwrap().len());

    // An accessor for the wrong type is None, never an error, and that holds
    // for the mut ones too.
    println!("object as array {}", list.as_object_mut().is_none());
    println!("array as object {}", request.as_array_mut().is_none());
    println!("null as object {}", Value::Null.as_object_mut().is_none());

    // A whole object printed back out, keys in alphabetical order.
    let mut stamped: Value = serde_json::from_str(r#"{"branch":"6000"}"#).unwrap();
    if let Some(object) = stamped.as_object_mut() {
        object.insert("clean".to_string(), Value::Bool(false));
    }
    println!("stamped {}", serde_json::to_string(&stamped).unwrap());

    // A value built out of its parts instead of parsed.
    let built = Value::Array(vec![Value::String("x".to_string()), Value::Bool(false)]);
    println!("built {}", serde_json::to_string(&built).unwrap());
}
