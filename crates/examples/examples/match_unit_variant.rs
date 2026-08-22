#!/usr/bin/env rust

// A bare unit variant pattern like `None` or an imported `Red` must be refutable, not a binding. A
// leading `None =>` must not swallow a `Some`. A lowercase ident is still a catch all.

use serde_json::Value;

use Color::{Blue, Green, Red};

#[derive(Debug)]
enum Color {
    Red,
    Green,
    Blue,
}

// imported unit variants, the leading arm first
fn bare_name(c: &Color) -> &str {
    match c {
        Red => "red",
        Green => "green",
        Blue => "blue",
    }
}

// a lowercase ident still binds as a catch all
fn is_red(c: &Color) -> String {
    match c {
        Red => "yes".to_string(),
        other => format!("no, {}", bare_name(other)),
    }
}

// The exact shape that regressed. `as_str` avoids displaying a `Value`, its text differs between
// compiled and interpreted for a json string.
fn field(data: &Value, key: &str) -> String {
    match data.get(key) {
        None => "absent".to_string(),
        Some(v) => format!("present {}", v.as_str().unwrap_or("?")),
    }
}

fn main() {
    for c in [Red, Green, Blue] {
        println!("bare {}", bare_name(&c));
    }

    println!("{}", is_red(&Red));
    println!("{}", is_red(&Green));

    let data: Value = serde_json::from_str(r#"{"a":"x"}"#).unwrap();
    println!("{}", field(&data, "a"));
    println!("{}", field(&data, "b"));

    let opts: [Option<i64>; 2] = [None, Some(7)];
    for opt in opts {
        let msg = match opt {
            None => "none".to_string(),
            Some(n) => format!("some {n}"),
        };
        println!("{msg}");
    }
}
