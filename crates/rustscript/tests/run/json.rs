use pretty_assertions::assert_eq;

use super::common::run;

#[test]
fn serde_json_roundtrip() {
    let out = run(r#"
use serde::Serialize;
#[derive(Serialize)]
struct Item { id: i64, name: String }
fn main() -> anyhow::Result<()> {
    let item = Item { id: 7, name: "gadget".to_string() };
    let json = serde_json::to_string(&item)?;
    println!("{json}");
    let parsed = serde_json::from_str(&json)?;
    let id = parsed["id"].clone();
    println!("{id:?}");
    Ok(())
}
"#);
    assert_eq!(out, "{\"id\":7,\"name\":\"gadget\"}\n7\n");
}

#[test]
fn typed_serde_deserialize() {
    let out = run(r##"
use serde::Deserialize;
#[derive(Deserialize)]
struct Point { x: i64, y: i64 }
fn main() -> anyhow::Result<()> {
    let p: Point = serde_json::from_str(r#"{"x":3,"y":4}"#)?;
    println!("{} {}", p.x, p.y);
    let list: Vec<i64> = serde_json::from_str("[1,2,3]")?;
    println!("{:?}", list);
    Ok(())
}
"##);
    assert_eq!(out, "3 4\n[1, 2, 3]\n");
}

#[test]
fn serde_rename_on_serialize_and_to_value() {
    let out = run(r##"
use serde::Serialize;
#[derive(Serialize)]
struct StatusLine {
    #[serde(rename = "type")]
    kind: String,
    command: String,
}
fn main() -> anyhow::Result<()> {
    let line = StatusLine { kind: "command".to_string(), command: "bun x.ts".to_string() };
    let flat = serde_json::to_string(&line)?;
    println!("{flat}");
    let mut data = serde_json::from_str::<serde_json::Value>(r#"{"theme":"light"}"#)?;
    data["statusLine"] = serde_json::to_value(line)?;
    let pretty = serde_json::to_string_pretty(&data)?;
    println!("{pretty}");
    Ok(())
}
"##);
    assert!(
        out.contains(r#""type":"command""#),
        "serialize missing rename: {out}"
    );
    assert!(
        out.contains(r#""type": "command""#),
        "to_value missing rename: {out}"
    );
    assert!(!out.contains("kind"), "raw field name leaked: {out}");
}

#[test]
fn json_float_integer_accessors_are_none() {
    // a json float like `used_percentage: 4.4`, the integer accessors must return `None` on it
    // like real serde_json
    let out = run(r##"
use serde_json::Value;
fn main() {
    let cw: Value = serde_json::from_str(r#"{"used_percentage":4.4,"size":7}"#).unwrap();
    let derived = 42;
    let pct = cw.get("used_percentage").and_then(Value::as_i64).unwrap_or(derived);
    println!("pct {pct}");
    println!("u64 {:?}", cw.get("used_percentage").and_then(Value::as_u64));
    println!("int {:?}", cw.get("size").and_then(Value::as_i64));
}
"##);
    assert_eq!(out, "pct 42\nu64 None\nint Some(7)\n");
}
