// A parsed `serde_json::Value` matched against its own variant patterns. The
// interpreter stores a parsed json as native values, a string as a Str, a number
// as an Int, an object as a Map, so matching `Value::String(s)` and friends has
// to recognize those. This mirrors the shape of a real dotted-path field reader.

use serde_json::Value;

fn scalar_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        Value::Array(a) => format!("[{} items]", a.len()),
        Value::Object(o) => format!("{{{} keys}}", o.len()),
    }
}

fn field_at(root: &Value, path: &str) -> String {
    let mut cur = root;
    for seg in path.split('.') {
        match cur.get(seg) {
            Some(next) => cur = next,
            None => return String::from("<absent>"),
        }
    }
    scalar_text(cur)
}

fn main() {
    let d: Value = serde_json::from_str(
        r#"{"summary":"hello","count":42,"done":true,"tags":["a","b"],"meta":{"name":"x"},"note":null}"#,
    )
    .unwrap();

    for key in ["summary", "count", "done", "tags", "meta", "note"] {
        println!("{key} = {}", scalar_text(d.get(key).unwrap()));
    }

    println!("meta.name = {}", field_at(&d, "meta.name"));
    println!("meta.missing = {}", field_at(&d, "meta.missing"));
}
