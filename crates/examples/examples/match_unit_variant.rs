#!/usr/bin/env rust

// A bare unit-variant pattern like None, or an imported enum variant like Red, must be refutable,
// not an always-true binding. The trap is the first arm: with a broken lowering a leading `None =>`
// swallows a Some value, and a leading `Red =>` swallows every color. A lowercase ident in the same
// spot is a real binding and must still act as a catch-all.

use serde_json::Value;

use Color::{Blue, Green, Red};

#[derive(Debug)]
enum Color {
    Red,
    Green,
    Blue,
}

// Bare unit variants, the leading arm first. Real Rust treats these as variant patterns because they
// are imported, so every arm is reachable.
fn bare_name(c: &Color) -> &str {
    match c {
        Red => "red",
        Green => "green",
        Blue => "blue",
    }
}

// A lowercase ident after a variant arm still binds as a catch-all.
fn is_red(c: &Color) -> String {
    match c {
        Red => "yes".to_string(),
        other => format!("no, {}", bare_name(other)),
    }
}

// None first, then Some: the exact shape that regressed. Uses as_str so no Value is Displayed, whose
// text differs between the compiled and interpreted engines for a json string.
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
