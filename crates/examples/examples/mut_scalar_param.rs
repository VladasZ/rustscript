#!/usr/bin/env rust

// `*n += 1` on a `&mut` scalar parameter. A scalar arrives as a copy and the write rides the
// `&mut` argument writeback home.

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
