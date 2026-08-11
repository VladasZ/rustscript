#!/usr/bin/env rust

// A `&mut` scalar parameter must hand its mutations back to the caller.
// `*n += 1` inside a callee used to fail with "assignment through a
// non-reference value". A scalar arrives as a plain copy rather than a
// reference, so the write goes into the parameter register and rides the
// caller's `&mut` argument writeback home.

fn bump(n: &mut i64, by: i64) {
    *n += by;
}

fn reset(n: &mut i64) {
    *n = 0;
}

fn rename(name: &mut String, to: &str) {
    *name = to.to_string();
}

fn main() {
    let mut n = 40;
    bump(&mut n, 2);
    println!("bumped: {n}");

    for _ in 0..3 {
        bump(&mut n, 10);
    }
    println!("looped: {n}");

    reset(&mut n);
    println!("reset: {n}");

    let mut name = "old".to_string();
    rename(&mut name, "new");
    println!("renamed: {name}");
}
