#!/usr/bin/env rust

fn fib(n: u64) -> u64 {
    if n < 2 {
        n
    } else {
        fib(n - 1) + fib(n - 2)
    }
}

fn main() {
    for i in 0..10 {
        println!("fib({i}) = {}", fib(i));
    }
}
