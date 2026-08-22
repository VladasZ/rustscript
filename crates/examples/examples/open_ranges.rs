#!/usr/bin/env rust

// The interpreter once refused `line.get(3..)` while `line[3..]` worked.

fn main() {
    let s = "AM mod/file.cpp";

    // The argument position.
    println!("get open [{}]", s.get(3..).unwrap_or(""));
    println!("get both [{}]", s.get(0..2).unwrap_or(""));
    println!("get inclusive [{}]", s.get(0..=1).unwrap_or(""));

    // Out of bounds is None.
    println!("get past end {}", s.get(99..).is_none());

    // The index position.
    println!("index open [{}]", &s[3..]);
    println!("index from start [{}]", &s[..2]);

    // Annotated so this stays a Vec, clippy would ask for an array.
    let v: Vec<i64> = vec![10, 20, 30, 40];
    let tail = &v[2..];
    println!("vec open {} {}", tail.len(), tail[1]);

    // Through a variable, so the sentinel survives a move.
    let r = 1..;
    println!("stored open [{}]", s.get(r).unwrap_or(""));
}
