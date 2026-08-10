#!/usr/bin/env rust

//! `collect` into a `HashMap` or `HashSet`, the map entry API writing through
//! `or_insert`, and the consuming `into_keys` / `into_values` accessors.

use std::collections::{HashMap, HashSet};

fn scores() -> HashMap<String, i64> {
    vec![("ada", 3i64), ("bob", 5i64)]
        .into_iter()
        .map(|(name, score)| (name.to_string(), score))
        .collect()
}

fn main() {
    // Collect into a map through the function's return type.
    let mut tally = scores();

    // The entry API answers a mutable slot, so `+=` lands in the map.
    *tally.entry(String::from("ada")).or_insert(0) += 10;
    *tally.entry(String::from("eve")).or_insert(7) += 1;
    *tally.entry(String::from("bob")).or_insert_with(|| 100) -= 2;

    let mut lines: Vec<String> = tally
        .iter()
        .map(|(name, score)| format!("{name}={score}"))
        .collect();
    lines.sort();
    println!("{}", lines.join(","));

    // The consuming accessors, sorted so the print order is deterministic.
    let mut names: Vec<String> = scores().into_keys().collect();
    names.sort();
    println!("{names:?}");
    let mut points: Vec<i64> = scores().into_values().collect();
    points.sort_unstable();
    println!("{points:?}");

    // Collect into a map through a `let` annotation and a turbofish.
    let squares: HashMap<i64, i64> = vec![1i64, 2, 3].into_iter().map(|n| (n, n * n)).collect();
    println!("{:?}", squares.get(&3));
    let doubled = vec![4i64, 5]
        .into_iter()
        .map(|n| (n, n * 2))
        .collect::<HashMap<i64, i64>>();
    println!("{:?}", doubled.get(&5));

    // Collect into a set, duplicates fold away.
    let unique: HashSet<i64> = vec![1i64, 2, 2, 3, 3, 3].into_iter().collect();
    println!("{}", unique.len());
    let evens = vec![2i64, 4, 4, 6].into_iter().collect::<HashSet<i64>>();
    println!("{}", evens.len());
}
