#!/usr/bin/env rust

//! `std::iter::empty` and `std::iter::once` are lazy iterators like any other, so they chain,
//! collect and drive a `for` loop. The coverage gate reports them when the bridge lacks them,
//! even on a branch that never runs.

fn fallback(values: &[i32]) -> Vec<i32> {
    if values.is_empty() {
        std::iter::empty().collect()
    } else {
        values.to_vec()
    }
}

fn main() {
    let none: Vec<i32> = std::iter::empty().collect();
    println!("{none:?} {}", none.len());
    println!("{:?}", fallback(&[]));
    println!("{:?}", fallback(&[4, 5]));

    let one: Vec<&str> = std::iter::once("first").collect();
    println!("{one:?}");
    let chained: Vec<u8> = std::iter::once(1u8).chain(vec![2, 3]).collect();
    println!("{chained:?}");
    let total: u64 = std::iter::empty::<u64>().chain(std::iter::once(7)).sum();
    println!("{total}");
    for word in std::iter::once("loop") {
        println!("{word}");
    }
    println!("{}", std::iter::empty::<char>().count());
}
