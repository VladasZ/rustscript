#!/usr/bin/env rust

// String buffers are shared and copied on the first append, so a clone taken before a push must
// keep its own contents.

fn main() {
    let mut s = String::from("base");
    let snapshot = s.clone();
    s.push_str("-grown");
    s.push('!');
    println!("s = {s}");
    println!("snapshot = {snapshot}");

    let mut total = String::new();
    let mut checkpoints: Vec<String> = Vec::new();
    let mut i = 0;
    while i < 6 {
        total.push_str("ab");
        if i % 2 == 0 {
            checkpoints.push(total.clone());
        }
        i += 1;
    }
    println!("total = {total}");
    for c in &checkpoints {
        println!("checkpoint {c}");
    }
}
