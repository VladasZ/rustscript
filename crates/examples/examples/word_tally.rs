#!/usr/bin/env rust

// Group values with the HashMap entry API.

use std::collections::HashMap;

fn main() {
    let words = "red green red blue green red";
    let mut buckets: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, w) in words.split_whitespace().enumerate() {
        buckets.entry(w.to_string()).or_default().push(i);
    }
    for color in ["blue", "green", "red"] {
        let positions = buckets.get(color).unwrap();
        println!("{}: {} at {:?}", color, positions.len(), positions);
    }
}
