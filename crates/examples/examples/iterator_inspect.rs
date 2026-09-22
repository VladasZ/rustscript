#!/usr/bin/env rust

// The shape comes from tracing which lines of a log a pipeline keeps.

fn main() {
    let levels = ["info", "warn", "error", "warn", "debug"];

    // inspect borrows each item, prints, and passes it through unchanged
    let warnings = levels
        .iter()
        .inspect(|l| println!("seen {l}"))
        .filter(|l| **l == "warn")
        .count();
    println!("warnings {warnings}");

    // owned items, the print sits between two stages
    let total: i64 = vec![1, 2, 3]
        .into_iter()
        .inspect(|n| println!("take {n}"))
        .map(|n| n * 10)
        .sum();
    println!("total {total}");

    // reversed, inspect runs from the back like a double ended adapter
    let joined: Vec<i64> = (1..=3).inspect(|n| println!("back {n}")).rev().collect();
    println!("joined {joined:?}");

    // an empty source never calls the closure
    let empty: Vec<i64> = Vec::new();
    let seen = empty.iter().inspect(|_| println!("never")).count();
    println!("seen {seen}");
}
