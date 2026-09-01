#!/usr/bin/env rust

//! Building a JSON payload with `serde_json::Map`, the way an API call assembles its body.
//! `Map::new` was the first gap the thing corpus sweep found, every short grafts a crate that
//! calls it.

use serde_json::{Map, Value};

fn main() {
    let mut variables = Map::new();
    variables.insert("body".to_string(), Value::String("hello".to_string()));
    variables.insert("line".to_string(), Value::from(42));
    variables.insert("path".to_string(), Value::Null);

    let mut payload = Map::new();
    payload.insert("query".to_string(), Value::String("mutation".to_string()));
    payload.insert("variables".to_string(), Value::Object(variables));

    let body = Value::Object(payload);
    println!("{}", serde_json::to_string(&body).unwrap());
    println!("{}", body["variables"]["line"]);
    println!("{}", body["variables"]["path"].is_null());
}
