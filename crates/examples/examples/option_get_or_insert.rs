#!/usr/bin/env rust

//! `get_or_insert_with` hands out a binding into the option payload, so pushes through it land
//! in the option itself. The shlex loop in the teams skill builds words exactly this way.

fn main() {
    let mut cur: Option<String> = None;
    cur.get_or_insert_with(String::new).push('a');
    cur.get_or_insert_with(String::new).push('b');
    let buf = cur.get_or_insert_with(String::new);
    buf.push('c');
    println!("{cur:?}");

    let mut count: Option<i64> = None;
    *count.get_or_insert(0) += 5;
    *count.get_or_insert(100) += 1;
    println!("{count:?}");

    let mut words: Vec<String> = Vec::new();
    let mut word: Option<String> = None;
    for c in "one two".chars() {
        if c == ' ' {
            if let Some(w) = word.take() {
                words.push(w);
            }
        } else {
            word.get_or_insert_with(String::new).push(c);
        }
    }
    if let Some(w) = word.take() {
        words.push(w);
    }
    println!("{words:?}");
}
