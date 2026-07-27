#!/usr/bin/env rust

// The tokio engine's copy of json_value_edit, so both engines are held to the
// same serde_json Value behaviour: the mut accessors hand back the real
// container, and the variant constructors build a value out of its payload.

use serde_json::Value;

fn tag(value: &mut Value, console: &str) {
    if let Some(object) = value.as_object_mut() {
        object.insert("console".to_string(), Value::String(console.to_string()));
    }
}

#[tokio::main]
async fn main() {
    let mut request: Value = serde_json::from_str(r#"{"cursor":null,"timeout_ms":15000}"#).unwrap();
    tag(&mut request, "m48");
    println!(
        "console {}",
        request.get("console").unwrap().as_str().unwrap()
    );
    println!("keys {}", request.as_object().unwrap().len());

    let mut list: Value = serde_json::from_str(r#"["a","b"]"#).unwrap();
    if let Some(items) = list.as_array_mut() {
        items.push(Value::Bool(true));
    }
    println!("list {}", serde_json::to_string(&list).unwrap());
    println!("object as array {}", list.as_object_mut().is_none());
    println!("array as object {}", request.as_array_mut().is_none());
}
