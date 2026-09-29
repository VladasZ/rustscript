#!/usr/bin/env rust

use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write as _;

use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize, Debug)]
struct Event {
    id: u32,
    data: Value,
    tags: Vec<Value>,
    extra: BTreeMap<String, Value>,
    maybe: Option<Value>,
}

#[derive(Debug)]
struct Pair(u8, Value);

#[derive(Debug)]
struct Node {
    name: String,
    meta: Value,
    child: Option<Box<Node>>,
}

struct Shown {
    data: Value,
}

impl fmt::Display for Shown {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "shown {}", self.data["k"])
    }
}

fn main() {
    let text = r#"{"id":1,"data":{"a":[1,2.5,null]},"tags":["x",true],
        "extra":{"n":3,"zone":"eu"},"maybe":{"deep":{"x":1}}}"#;
    let event: Event = serde_json::from_str(text).unwrap();
    println!("{event:?}");
    println!("{} {}", event.id, event.data);
    println!("{event:#?}");
    println!("{:?}", event.tags);
    println!("{:?}", event.extra);
    println!("{:?}", event.maybe);

    let map: BTreeMap<String, Value> = serde_json::from_str(r#"{"n":3}"#).unwrap();
    println!("{map:?}");
    let list: Vec<Value> = vec![json!(1), json!("s"), json!(null)];
    println!("{list:?} {list:#?}");
    let tuple = (7, json!({"t": [1]}));
    println!("{tuple:?}");
    let pair = Pair(2, json!(false));
    println!("{pair:?} {} {}", pair.0, pair.1);

    let parsed: Result<Value, String> = Ok(json!([1, 2]));
    println!("{parsed:?}");
    let failed: Result<u8, Value> = Err(json!({"code": 4}));
    println!("{failed:?}");

    let node = Node {
        name: "root".to_string(),
        meta: json!({"k": 1}),
        child: None,
    };
    println!("{node:?}");
    println!("{} {} {}", node.name, node.meta, node.child.is_none());

    let shown = Shown {
        data: json!({"k": "v"}),
    };
    println!("{shown}");

    let plain = json!({"n": 3});
    println!("{plain:?} {plain}");

    let mut out = String::new();
    write!(out, "{:?} {}", plain, plain["n"]).unwrap();
    writeln!(out, " {map:?}").unwrap();
    print!("{out}");
}
