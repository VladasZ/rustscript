#!/usr/bin/env rust

// The serde_json accessors that hand back an Option, and the json null value.
// A json string is a plain String in the interpreter, so `as_str` gives it
// back as an already unwrapped Some. These are the shapes that have to keep
// behaving like a real Option anyway: match, if let, or_else, and the `?`
// operator, which passes an already unwrapped value through as its own Some.

use serde_json::Value;

fn dir_of(data: &Value) -> String {
    let workspace = data
        .get("workspace")
        .and_then(|w| w.get("current_dir"))
        .and_then(Value::as_str);
    match workspace {
        Some(dir) => dir.to_string(),
        None => data
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or("none")
            .to_string(),
    }
}

// The `?` chain over json accessors, get and as_str each answer an Option in
// real Rust while the interpreter sees plain values in the middle.
fn tag_of(data: &Value) -> Option<String> {
    Some(data.get("tag_name")?.as_str()?.to_string())
}

fn main() {
    let text = r#"{"workspace":{"current_dir":"/a/b"},"cwd":"/fallback"}"#;
    let full: Value = serde_json::from_str(text).unwrap();
    let bare: Value = serde_json::from_str(r#"{"cwd":"/fallback"}"#).unwrap();
    let empty: Value = serde_json::from_str("{}").unwrap();

    println!("full  {}", dir_of(&full));
    println!("bare  {}", dir_of(&bare));
    println!("empty {}", dir_of(&empty));

    if let Some(dir) = full.get("cwd").and_then(Value::as_str) {
        println!("if let {dir}");
    }

    let tagged: Value = serde_json::from_str(r#"{"tag_name":"fork-0.17.0-3"}"#).unwrap();
    let untagged: Value = serde_json::from_str(r#"{"other":1}"#).unwrap();
    let numbered: Value = serde_json::from_str(r#"{"tag_name":7}"#).unwrap();
    let nulled: Value = serde_json::from_str(r#"{"tag_name":null}"#).unwrap();
    println!("try tag  {:?}", tag_of(&tagged));
    println!("try none {:?}", tag_of(&untagged));
    println!("try num  {:?}", tag_of(&numbered));
    println!("try null {:?}", tag_of(&nulled));

    let picked = bare
        .get("workspace")
        .and_then(|w| w.get("current_dir"))
        .and_then(Value::as_str)
        .or_else(|| bare.get("cwd").and_then(Value::as_str))
        .unwrap_or("none");
    println!("or_else {picked}");

    // A missing branch falls back to the json null value, and the accessors
    // still answer on it instead of failing.
    let missing = empty.get("context").cloned().unwrap_or(Value::Null);
    println!("missing is_null {}", missing.is_null());
    println!(
        "missing nested  {}",
        missing.get("size").and_then(Value::as_i64).unwrap_or(-1)
    );

    let present = full.get("workspace").cloned().unwrap_or(Value::Null);
    println!("present is_null {}", present.is_null());
    println!(
        "present nested  {}",
        present
            .get("current_dir")
            .and_then(Value::as_str)
            .unwrap_or("none")
    );

    // A json float stays f64, so the integer accessors answer None on it even
    // for a whole value like 5.0, and the caller's fallback has to kick in.
    let nums: Value = serde_json::from_str(r#"{"pct":4.4,"whole":5.0,"count":7}"#).unwrap();
    let derived = 42;
    println!(
        "float as_i64 {}",
        nums.get("pct").and_then(Value::as_i64).unwrap_or(derived)
    );
    println!(
        "whole as_i64 {}",
        nums.get("whole").and_then(Value::as_i64).unwrap_or(derived)
    );
    println!(
        "int as_i64   {}",
        nums.get("count").and_then(Value::as_i64).unwrap_or(derived)
    );
    println!(
        "float as_f64 {}",
        nums.get("pct").and_then(Value::as_f64).unwrap_or(0.0)
    );
}
