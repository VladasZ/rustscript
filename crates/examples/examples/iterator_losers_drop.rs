#!/usr/bin/env rust

// The losers of `min`, `max` and `max_by_key` drop inside the call, and `into_keys` hands
// out an iterator that owns its items like `into_iter` does.

use std::collections::HashMap;
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct T(i64);
impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
fn main() {
    println!("-- into_iter min");
    let owned_min = vec![T(1), T(2)].into_iter().min();
    println!("owned_min {owned_min:?}");
    println!("-- map min");
    let mapped_min = vec![1, 2].into_iter().map(|_| T(3)).min().unwrap_or(T(9));
    println!("mapped_min {mapped_min:?}");
    println!("-- into_keys map min unwrap_or_default");
    let mut table: HashMap<usize, u8> = HashMap::new();
    table.insert(1, 0);
    table.insert(2, 0);
    let keyed_min = table.into_keys().map(|_| T(4)).min().unwrap_or(T(8));
    println!("keyed_min {keyed_min:?}");
    println!("-- max_by_key");
    let by_key = vec![T(5), T(6)].into_iter().max_by_key(|t| t.0);
    println!("by_key {by_key:?}");
    println!("end");
}
