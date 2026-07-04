#!/usr/bin/env rust

// Spawn worker threads and join their results. The interpreter runs them
// serially, so results still come back in order.

use std::thread;

fn main() {
    let mut handles = Vec::new();
    for i in 0..5 {
        handles.push(thread::spawn(move || i * i));
    }
    let mut total = 0;
    for h in handles {
        total += h.join().unwrap();
    }
    println!("sum of squares 0..5: {}", total);
}
