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

    // both stay lazy under `take`
    let ages = [31, 27];
    let paired: Vec<(String, i32)> = names
        .iter()
        .zip(ages.iter())
        .map(|(name, age)| (name.to_string(), *age))
        .collect();
    println!("{paired:?}");
    let joined: Vec<i64> = nums
        .iter()
        .copied()
        .chain(100..103)
        .filter(|n| n % 2 == 1)
        .collect();
    println!("{joined:?}");
    let indexed: Vec<(usize, char)> = (0..).zip("rust".chars()).take(3).collect();
    println!("{indexed:?}");
    let empty: Vec<(i64, i64)> = Vec::<i64>::new()
        .into_iter()
        .zip(Vec::<i64>::new())
        .collect();
    println!("{empty:?}");
}
