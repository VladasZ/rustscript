#!/usr/bin/env rust

//! A value stored into a container keeps its real width, so a saturated
//! usize survives a push and big u64 values sort by value.

fn opaque(v: u64) -> u64 {
    v
}

fn opaque_usize(v: usize) -> usize {
    v
}

fn main() {
    let mut values: Vec<usize> = vec![opaque_usize(2)];
    let big = opaque_usize(18_446_744_073_709_551_614);
    values.push(big.saturating_add(opaque_usize(3)));
    println!("pushed: {values:?}");

    // Two u64 values past i64::MAX still sort by value.
    let mut ordered: Vec<u64> = vec![opaque(u64::MAX)];
    ordered.push(opaque(18_446_744_073_709_551_614));
    ordered.sort_unstable();
    println!("sorted: {ordered:?}");

    let mut map: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    map.insert(String::from("big"), opaque(18_446_744_073_709_551_613));
    println!("stored: {:?}", map.get("big"));
}
