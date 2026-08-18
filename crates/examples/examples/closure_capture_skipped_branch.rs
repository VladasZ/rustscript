#!/usr/bin/env rust

// A closure that mutably captures a local shares the local through a cell
// built when the closure value is created. A branch that never runs creates
// no closure and no cell, so later reads of the local still answer with the
// value the local holds on its own.

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

    // The same local read from both a skipped closure and the branch that
    // does run. `rotate_left` reads its receiver, and the read stays right
    // whether the closure holding it ever ran or not.
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

    // A skipped closure in a loop leaves every later iteration reading the
    // local directly.
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
