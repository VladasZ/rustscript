#!/usr/bin/env rust

// A branch that never runs creates no closure and no capture cell, so later
// reads of the local must still work.

fn main() {
    let mut total = 3i64;
    let empty: Vec<i64> = Vec::new();
    if !empty.is_empty() {
        let mut add = |value: i64| total += value;
        add(100);
    }
    println!("skipped {total}");

    let mut count = 0i64;
    let taken = empty.is_empty();
    if taken {
        let mut bump = || count += 1;
        bump();
        bump();
    }
    println!("taken {count}");

    // The same local read from a skipped closure and the branch that runs.
    let seed = 9i64;
    let shifts: Vec<u32> = Vec::new();
    let rotated: Vec<i64> = if shifts.is_empty() {
        vec![seed]
    } else {
        shifts
            .iter()
            .map(|shift| seed.rotate_left(*shift))
            .collect()
    };
    println!("rotated {rotated:?} seed {seed}");

    // A skipped closure in a loop.
    let mut sum = 0i64;
    for step in 1..4i64 {
        if step > 10 {
            let mut grow = || sum += 1000;
            grow();
        }
        sum += step;
    }
    println!("sum {sum}");
}
