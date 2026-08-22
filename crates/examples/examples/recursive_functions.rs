#!/usr/bin/env rust

//! Self recursive scalar functions run as function plans, see `interpreter/scalar_fn.rs`.

fn fib(n: u64) -> u64 {
    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

// deep recursion, so the plan's frame stack grows past 1 window
fn sum_to(n: i64, acc: i64) -> i64 {
    if n == 0 { acc } else { sum_to(n - 1, acc + n) }
}

// a numeric method and a 3 way branch inside the plan
fn collatz_len(n: u64, steps: u64) -> u64 {
    if n == 1 {
        steps
    } else if n.is_multiple_of(2) {
        collatz_len(n / 2, steps + 1)
    } else {
        collatz_len(n * 3 + 1, steps + 1)
    }
}

// mutual recursion stays on the generic path
fn is_even(n: u64) -> bool {
    if n == 0 { true } else { is_odd(n - 1) }
}

fn is_odd(n: u64) -> bool {
    if n == 0 { false } else { is_even(n - 1) }
}

fn main() {
    println!("fib(30) = {}", fib(30));
    println!("gcd(1071, 462) = {}", gcd(1_071, 462));
    println!("sum_to(10000) = {}", sum_to(10_000, 0));
    println!("collatz_len(27) = {}", collatz_len(27, 0));
    println!("is_even(10) = {}", is_even(10));
    println!("is_odd(7) = {}", is_odd(7));
}
