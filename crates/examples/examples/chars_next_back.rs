#!/usr/bin/env rust

//! `Chars` is double ended. `next_back` takes from the end, `as_str` is what is left between
//! both ends, and `rev` stays lazy.

fn main() {
    let mut chars = "héllo".chars();
    println!(
        "{:?} {:?} {:?}",
        chars.next_back(),
        chars.next(),
        chars.as_str()
    );
    println!("{:?}", chars.rev().collect::<String>());

    let mut both = "abc".chars();
    println!(
        "{:?} {:?} {:?} {:?} {:?}",
        both.next(),
        both.next_back(),
        both.next(),
        both.next_back(),
        both.next()
    );
    println!("{:?}", "über".chars().rev().take(2).collect::<Vec<char>>());
    println!("{:?}", "a-b".chars().rev().position(|ch| ch == '-'));
    println!("{:?}", "xyz".chars().last());
}
