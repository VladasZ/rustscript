#!/usr/bin/env rust

// Open-ended ranges in every position, not just inside `[]`. The shape comes
// from a real script that read git porcelain lines with `line.get(3..)`, which
// the interpreter refused while `line[3..]` worked. compile_range now emits
// the same open-end sentinel for both, and `str::get` reads it as len.

fn main() {
    let s = "AM mod/file.cpp";

    // The argument position, the case that used to be refused.
    println!("get open [{}]", s.get(3..).unwrap_or(""));
    println!("get both [{}]", s.get(0..2).unwrap_or(""));
    println!("get inclusive [{}]", s.get(0..=1).unwrap_or(""));

    // Out of bounds answers None exactly like real `str::get`.
    println!("get past end {}", s.get(99..).is_none());

    // The index position keeps working the same as before.
    println!("index open [{}]", &s[3..]);
    println!("index from start [{}]", &s[..2]);

    let v = vec![10, 20, 30, 40];
    let tail = &v[2..];
    println!("vec open {} {}", tail.len(), tail[1]);

    // An open range fed through a variable, so the sentinel survives a move.
    let r = 1..;
    println!("stored open [{}]", s.get(r).unwrap_or(""));
}
