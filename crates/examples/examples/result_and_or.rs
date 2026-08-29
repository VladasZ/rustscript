#!/usr/bin/env rust

// `and`, `or` and `map_or_else` have to carry the ok type through, otherwise a later
// `unwrap_or_default` has no type to build a default from.

use std::collections::HashMap;

fn table(key: &str, value: i64) -> HashMap<String, i64> {
    let mut map = HashMap::new();
    map.insert(key.to_string(), value);
    map
}

fn main() {
    let chained = Ok::<HashMap<String, i64>, String>(table("a", 1))
        .and(Ok::<HashMap<String, i64>, String>(table("b", 2)))
        .unwrap_or(table("c", 3));
    println!("b = {}", chained.get("b").copied().unwrap_or_default());
    println!(
        "missing = {}",
        chained.get("a").copied().unwrap_or_default()
    );

    let recovered = Err::<HashMap<String, i64>, String>(String::from("no"))
        .or(Ok::<HashMap<String, i64>, i64>(table("d", 4)))
        .unwrap_or_default();
    println!("d = {}", recovered.get("d").copied().unwrap_or_default());

    let counted = Ok::<HashMap<String, i64>, String>(table("e", 5))
        .map_or_else(|error| error.len(), |map| map.len());
    println!("counted = {counted}");

    let described = Err::<HashMap<String, i64>, String>(String::from("boom"))
        .map_or_else(|error| error.len(), |map| map.len());
    println!("described = {described}");
}
