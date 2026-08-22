#!/usr/bin/env rust

// A captured `let r = &mut v` alias must resolve across the frame boundary.

fn main() {
    let mut v = vec![1, 2];
    let r = &mut v;
    let mut push = || r.push(3);
    push();
    push();
    println!("{v:?}");

    let mut n = 5i64;
    let r = &mut n;
    let mut bump = || *r += 1;
    bump();
    bump();
    println!("{n}");

    let mut total = 0i64;
    let r = &mut total;
    let mut add = |x: i64| *r += x;
    add(10);
    add(32);
    println!("{total}");

    let mut text = String::from("a");
    let r = &mut text;
    let read = || r.len();
    println!("{} {}", read(), text);
}
