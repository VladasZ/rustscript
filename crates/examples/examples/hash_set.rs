#!/usr/bin/env rust

use std::collections::{BTreeSet, HashSet};

fn main() {
    let mut seen: HashSet<String> = HashSet::new();
    println!("{}", seen.insert("LL-472".to_string()));
    println!("{}", seen.insert("LL-938".to_string()));
    println!("{}", seen.insert("LL-472".to_string()));
    println!("{}", seen.contains("LL-938"));
    println!("{}", seen.contains("LL-1"));
    println!("{}", seen.len());

    // Iteration order is unspecified in real Rust, so only order-insensitive
    // reads compare byte for byte against the compiled run.
    let mut total = 0;
    for key in &seen {
        total += key.len();
    }
    println!("{total}");

    println!("{}", seen.remove("LL-938"));
    println!("{}", seen.remove("LL-938"));

    let mut keys: Vec<String> = seen.into_iter().collect();
    keys.sort();
    println!("{}", keys.join(","));

    let nums: HashSet<i64> = vec![1, 2, 2, 3].into_iter().collect();
    println!("{}", nums.len());
    println!("{}", nums.contains(&3));

    let mut tags: BTreeSet<&str> = BTreeSet::new();
    println!("{}", tags.insert("blue"));
    println!("{}", tags.is_empty());
    println!("{}", tags.contains("blue"));
}
