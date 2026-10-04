#!/usr/bin/env rust

//! `as_array::<N>()` on a slice is `Some` only when the slice has exactly `N` items. The json
//! accessor of the same name takes no length.

fn main() {
    let v: Vec<u8> = (1..=2).collect();
    println!("{:?}", v.as_array::<2>().map(|a| a.len()));
    println!("{:?}", v.as_array::<3>().map(|a| a.len()));
    println!("{:?}", v.as_array::<0>().is_some());
    let empty: Vec<String> = Vec::new();
    println!("{:?}", empty.as_array::<0>().is_some());
    if let Some([a, b]) = v.as_array::<2>() {
        println!("{a} {b}");
    }

    let value: serde_json::Value = serde_json::from_str("[1, 2, 3]").expect("valid json");
    println!("{:?}", value.as_array().map(Vec::len));
}
