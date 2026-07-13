#!/usr/bin/env rust

fn main() {
    let nums: Vec<i64> = (1..=10).collect();
    let sum_sq_even: i64 = nums.iter().filter(|n| *n % 2 == 0).map(|n| n * n).sum();
    println!("sum of squares of evens: {sum_sq_even}");

    let names = ["alice", "bob", "carol"];
    let shout: Vec<String> = names.iter().map(|n| n.to_uppercase()).collect();
    println!("{shout:?}");

    let any_long = names.iter().any(|n| n.len() > 4);
    let all_short = names.iter().all(|n| n.len() < 10);
    println!("any long: {any_long}, all short: {all_short}");
}
