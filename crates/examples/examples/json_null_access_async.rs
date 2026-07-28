#!/usr/bin/env rust

// The tokio engine's copy of json_null_access, so both engines are held to the
// same serde_json behaviour on a null. A json null is Option::None inside the
// interpreter, and the tokio engine used to fail with an unknown method there.
// The myip script hit it for real: one address service answered nothing, the
// parse fell back to Value::Null, and reading a field off it aborted the run.

use serde_json::Value;

fn text_of(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(found) => found.as_str().unwrap_or("").to_string(),
        None => String::new(),
    }
}

fn bool_of(v: &Value, key: &str) -> bool {
    match v.get(key) {
        Some(found) => found.as_bool().unwrap_or(false),
        None => false,
    }
}

// Every serde type test, printed as the names that answered true.
fn kind_of(v: &Value) -> String {
    let mut names: Vec<String> = Vec::new();
    if v.is_object() {
        names.push("object".to_string());
    }
    if v.is_array() {
        names.push("array".to_string());
    }
    if v.is_string() {
        names.push("string".to_string());
    }
    if v.is_boolean() {
        names.push("boolean".to_string());
    }
    if v.is_number() {
        names.push("number".to_string());
    }
    if v.is_i64() {
        names.push("i64".to_string());
    }
    if v.is_u64() {
        names.push("u64".to_string());
    }
    if v.is_f64() {
        names.push("f64".to_string());
    }
    if v.is_null() {
        names.push("null".to_string());
    }
    names.join(",")
}

#[tokio::main]
async fn main() {
    let text = r#"{"ip":"86.100.76.6","proxy":true,"port":8080,"share":0.5}"#;
    let live: Value = serde_json::from_str(text).unwrap();
    // What a failed request leaves behind, an empty body that does not parse.
    let dead: Value = serde_json::from_str("").unwrap_or(Value::Null);

    println!("live ip    [{}]", text_of(&live, "ip"));
    println!("dead ip    [{}]", text_of(&dead, "ip"));
    println!("live proxy {}", bool_of(&live, "proxy"));
    println!("dead proxy {}", bool_of(&dead, "proxy"));

    println!("null get       {}", dead.get("ip").is_none());
    println!(
        "null nested    {}",
        dead.get("a").and_then(|v| v.get("b")).is_none()
    );
    println!("null as_str    {}", dead.as_str().unwrap_or("none"));
    println!("null as_i64    {}", dead.as_i64().unwrap_or(-1));
    println!("null as_u64    {}", dead.as_u64().unwrap_or(0));
    println!("null as_bool   {}", dead.as_bool().unwrap_or(false));
    println!("null as_f64    {}", dead.as_f64().is_none());
    println!("null as_array  {}", dead.as_array().is_none());
    println!("null as_object {}", dead.as_object().is_none());

    println!("kind object {}", kind_of(&live));
    println!("kind null   {}", kind_of(&dead));
    println!("kind string {}", kind_of(live.get("ip").unwrap()));
    println!("kind bool   {}", kind_of(live.get("proxy").unwrap()));
    println!("kind int    {}", kind_of(live.get("port").unwrap()));
    println!("kind float  {}", kind_of(live.get("share").unwrap()));

    // A json array reads by index, and a missing index is None like serde.
    let list: Value = serde_json::from_str(r#"["first","second"]"#).unwrap();
    println!("kind array  {}", kind_of(&list));
    println!(
        "index 1     {}",
        list.get(1).and_then(Value::as_str).unwrap_or("none")
    );
    println!("index 9     {}", list.get(9).is_none());
}
