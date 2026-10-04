#!/usr/bin/env rust

//! A typed read takes only what the real type takes. Every other input is the error serde
//! gives, and `{:?}` of that error is the real one, `Error("..", line: 1, column: 8)`. An
//! `Option` keeps every level, inside another `Option` and inside a `Vec`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
struct Config {
    port: u8,
    // the nested option is what the read has to keep
    #[allow(clippy::option_option)]
    name: Option<Option<String>>,
    levels: Vec<Option<i16>>,
    #[serde(default)]
    quiet: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct Tags {
    tags: Vec<u8>,
}

fn read(label: &str, text: &str) {
    println!("{label} {:?}", serde_json::from_str::<Config>(text));
}

fn main() {
    read(
        "full",
        r#"{"port": 1, "name": "x", "levels": [1, null, 3]}"#,
    );
    read("null name", r#"{"port": 1, "name": null, "levels": []}"#);
    read("null", "null");
    read("number", "3");
    read("text", r#""text""#);
    read("empty", "");
    read("open", "{");
    read("too big", r#"{"port": 300, "levels": []}"#);
    read("map for a list", r#"{"port": 1, "levels": {}}"#);
    read("number for a list", r#"{"port": 1, "levels": 3}"#);
    read("missing", "{\n  \"levels\": []\n}");
    read("twice", r#"{"port": 1, "levels": [], "port": 2}"#);
    // a derived struct also reads from a list, the fields in order
    read("list", r#"[1, "x", [2]]"#);
    read("list with all", r#"[1, "x", [2], true]"#);
    read("short list", "[1]");
    read("long list", r#"[1, "x", [2], true, 5]"#);

    println!("{:?}", serde_json::from_str::<Tags>(r#"{"tags": "abc"}"#));
    println!("{:?}", serde_json::from_str::<Tags>("[]"));
    println!("{:?}", serde_json::from_str::<Vec<Option<u8>>>("[1, null]"));
    println!("{:?}", serde_json::from_str::<Option<Tags>>("null"));
    println!("{:?}", serde_json::from_str::<Option<Vec<u8>>>("[1]"));
    println!("{:?}", serde_json::from_str::<BTreeMap<String, u8>>("[1]"));
    println!("{:?}", serde_json::from_str::<Vec<u8>>(r#"{"a": 1}"#));
    println!(
        "{:?}",
        serde_json::from_str::<BTreeMap<String, Option<u8>>>(r#"{"a": null, "b": 2}"#)
    );

    match serde_json::from_str::<Config>(r#"{"port": 1, "name": "x", "levels": [1, null]}"#) {
        Ok(config) => println!("{:?}", serde_json::to_string(&config)),
        Err(error) => println!("{error}"),
    }
    match serde_json::from_str::<Tags>(r#"{"tags": [1, 2]}"#) {
        Ok(tags) => println!("{:?}", serde_json::to_string(&tags)),
        Err(error) => println!("{error}"),
    }

    let error = serde_json::from_str::<Config>("{").unwrap_err();
    println!("{error} | {error:?} | {}", error.to_string().len());
    let untyped: Result<serde_json::Value, serde_json::Error> = serde_json::from_str("{1}");
    println!("{untyped:?}");

    println!("{:?}", toml::from_str::<Tags>("tags = 3"));
    println!("{:?}", toml::from_str::<Tags>("tags = [1, 2]"));
}
