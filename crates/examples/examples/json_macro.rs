#!/usr/bin/env rust


use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;

// fields and keys are in sorted order, a compiled `serde_json` without `preserve_order` sorts
// object keys and the interpreter keeps insertion order
#[derive(Serialize)]
struct User {
    boss: Option<Box<User>>,
    id: u32,
    tags: Vec<String>,
    #[serde(rename = "userName")]
    name: String,
}

fn main() {
    let name = "ann";
    let n = 3;
    let key = "dynamic";
    let items = vec![1, 2, 3];
    let user = User {
        boss: None,
        id: 7,
        tags: vec!["x".into()],
        name: "bob".into(),
    };
    let mut extra = BTreeMap::new();
    extra.insert("b", 2.5);
    extra.insert("a", -1.0);
    let v = json!({
        "count": n * 2,
        key: format!("{}!", name.to_uppercase()),
        "expr": if n > 2 { "big" } else { "small" },
        "extra": extra,
        "items": items,
        "list": [1, "two", null, [3, {"deep": false}]],
        "maybe": Option::<i32>::None,
        "name": name,
        "neg": -4,
        "nested": { "a": { "b": [] }, "e": {} },
        "nothing": null,
        "ok": true,
        "path": std::f64::consts::PI > 3.0,
        "some": Some("s"),
        "user": user,
    });
    println!("{}", serde_json::to_string(&v).unwrap());
    println!(
        "{}",
        serde_json::to_string_pretty(&json!([1, {"k": [true]}])).unwrap()
    );
    println!("{} {}", v["count"], v["list"][3][1]["deep"]);
    println!("{}", serde_json::to_string(&json!(null)).unwrap());
    println!("{}", serde_json::to_string(&json!("plain")).unwrap());
    let arr = json!(items.iter().map(|x| x * 10).collect::<Vec<_>>());
    println!("{}", serde_json::to_string(&arr).unwrap());
}
