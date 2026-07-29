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

    // serde answers the integer tests by range, not by "it is an integer", so
    // a negative number is not a u64 and one past i64::MAX is not an i64.
    let nums: Value =
        serde_json::from_str(r#"{"neg":-3,"pos":7,"big":18446744073709551615}"#).unwrap();
    let neg = nums.get("neg").unwrap();
    let big = nums.get("big").unwrap();
    println!("kind neg    {}", kind_of(neg));
    println!("kind pos    {}", kind_of(nums.get("pos").unwrap()));
    println!("kind big    {}", kind_of(big));
    println!("neg as_u64  {}", neg.as_u64().is_none());
    println!("neg as_i64  {}", neg.as_i64().unwrap_or(0));
    println!("big as_i64  {}", big.as_i64().is_none());
    println!("big as_u64  {}", big.as_u64().unwrap_or(0));
    println!("big as_f64  {}", big.as_f64().unwrap_or(0.0));

    // Json pointer, RFC 6901. A key with a slash or a tilde is escaped, an
    // index with a leading zero is not an index, and a pointer that leaves
    // the tree is None rather than an error.
    let tree: Value =
        serde_json::from_str(r#"{"a":{"b c":[10,{"d":"deep"}]},"e/f":1,"g~h":2}"#).unwrap();
    println!(
        "ptr deep    {}",
        tree.pointer("/a/b c/1/d")
            .and_then(Value::as_str)
            .unwrap_or("none")
    );
    println!(
        "ptr index   {}",
        tree.pointer("/a/b c/0")
            .and_then(Value::as_i64)
            .unwrap_or(-1)
    );
    println!(
        "ptr escape  {}",
        tree.pointer("/e~1f").and_then(Value::as_i64).unwrap_or(-1)
    );
    println!(
        "ptr tilde   {}",
        tree.pointer("/g~0h").and_then(Value::as_i64).unwrap_or(-1)
    );
    println!("ptr whole   {}", tree.pointer("").is_some());
    println!("ptr no slash {}", tree.pointer("a/b").is_none());
    println!("ptr missing {}", tree.pointer("/a/zz").is_none());
    println!("ptr past end {}", tree.pointer("/a/b c/9").is_none());
    println!("ptr zeroed  {}", tree.pointer("/a/b c/01").is_none());
    println!("ptr on null {}", dead.pointer("/a").is_none());

    let mut owned: Value = serde_json::from_str(r#"{"a":{"n":1}}"#).unwrap();
    if let Some(found) = owned.pointer_mut("/a")
        && let Some(object) = found.as_object_mut()
    {
        object.insert("added".to_string(), Value::Bool(true));
    }
    println!(
        "ptr mut     {}",
        owned
            .pointer("/a/added")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    );

    // `get` on a value that turned out to be a scalar answers None like serde,
    // instead of failing the way an unknown method would. Every shape a json
    // value can be has to answer, which is the whole point of the check that
    // now guards this.
    for text in ["\"hi\"", "5", "4.5", "true", "null", "{}", "[1,2]"] {
        let shape: Value = serde_json::from_str(text).unwrap_or(Value::Null);
        println!("shape {text} get {}", text_of(&shape, "k").is_empty());
    }
    let pair: Value = serde_json::from_str("[1,2]").unwrap();
    println!("arr by index {:?}", pair.get(1).and_then(Value::as_i64));
    println!("arr by key   {}", pair.get("k").is_none());

    // `str::get` is the real slice method and keeps its own meaning, it is not
    // the json lookup.
    let word = "hello".to_string();
    println!("str slice    {:?}", word.get(0..2));
    println!("str past end {:?}", word.get(0..99));
    println!("str inclusive {:?}", word.get(0..=1));
}
