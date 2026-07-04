#!/usr/bin/env rust

// Read all of stdin and report a summary. With no input the counts are zero, so
// it finishes cleanly whether or not anything is piped in.

use std::io::{self, Read};

fn main() -> anyhow::Result<()> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let lines = input.lines().count();
    let words = input.split_whitespace().count();
    println!("lines: {}", lines);
    println!("words: {}", words);
    Ok(())
}
