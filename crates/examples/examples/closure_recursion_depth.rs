#!/usr/bin/env rust

//! Recursion that passes through a closure handed to an iterator adaptor. Each level used to nest
//! the whole VM on the host stack of a 2 MiB thread, so a tree walk aborted the process at depth
//! 1000 while compiled Rust went past 50000.

fn walk(depth: u64) -> u64 {
    if depth == 0 {
        return 0;
    }
    (0..1u64).map(|_| walk(depth - 1)).sum::<u64>() + 1
}

fn count(depth: u64) -> u64 {
    if depth == 0 {
        return 0;
    }
    let below: Vec<u64> = vec![depth - 1].into_iter().map(count).collect();
    below[0] + 1
}

fn main() {
    println!("{}", walk(5000));
    println!("{}", count(5000));
}
