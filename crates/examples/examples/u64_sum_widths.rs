#!/usr/bin/env rust

//! The eager vec path once clamped elements at `i64::MAX` before adding.

use std::collections::HashSet;

fn opaque(v: u64) -> u64 {
    v
}

fn main() {
    let big: u64 = opaque(17_387_282_529_756_548_797u64);

    // Lazy.
    let lazy: u64 = vec![1i64]
        .into_iter()
        .map(|_n| big.min(opaque(u64::MAX)))
        .sum::<u64>();
    println!("{lazy}");

    // Eager.
    let mut source: HashSet<i64> = HashSet::new();
    source.insert(0);
    let eager = source
        .into_iter()
        .map(|_n: i64| big.min(opaque(u64::MAX)))
        .sum::<u64>();
    println!("{eager}");
    println!("{}", u32::try_from(eager % 4_294_967_296).unwrap_or(0));
}
