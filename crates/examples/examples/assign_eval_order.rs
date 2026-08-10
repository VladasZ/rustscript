#!/usr/bin/env rust

//! An assignment evaluates its right operand before the place expression, in
//! plain and compound form alike. The prints make the order observable, so
//! the interpreted run must match compiled Rust line for line.

use std::collections::HashMap;

fn traced(tag: &str, value: i64) -> i64 {
    println!("eval {tag}");
    value
}

fn traced_index(tag: &str, value: usize) -> usize {
    println!("eval {tag}");
    value
}

fn main() {
    let mut tally: HashMap<String, i64> = HashMap::new();
    *tally.entry(String::from("k")).or_insert(traced("place", 1)) += traced("value", 2);
    println!("entry: {:?}", tally.get("k"));

    let mut items = vec![traced("init", 10)];
    items[traced_index("index", 0)] = traced("assigned", 20);
    println!("items: {items:?}");

    items[traced_index("index again", 0)] += traced("added", 5);
    println!("items: {items:?}");
}
