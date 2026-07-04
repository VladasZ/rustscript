#!/usr/bin/env rust

// Measure elapsed time and build durations. Only stable facts are printed.

use std::time::{Duration, Instant};

fn main() {
    let start = Instant::now();
    let mut total: u64 = 0;
    for i in 0..10_000 {
        total += i;
    }
    let elapsed = start.elapsed();
    println!("did work: {}", total > 0);
    println!("elapsed measured: {}", elapsed.as_secs() < 3600);

    let d = Duration::from_millis(1500);
    println!("1500ms as secs: {}", d.as_secs());
    println!("1500ms as millis: {}", d.as_millis());
}
