#!/usr/bin/env rust

use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    println!("program: {}", args[0]);
    println!("count: {}", args.len() - 1);
    for (i, a) in args.iter().skip(1).enumerate() {
        println!("arg {}: {}", i + 1, a);
    }
}
