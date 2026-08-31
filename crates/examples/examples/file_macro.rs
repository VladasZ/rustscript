#!/usr/bin/env rust

// `file!` gives the path that was handed to the compiler, or to the interpreter, so only the tail
// of it is the same in both.

fn main() {
    let path = file!();
    println!("names this script: {}", path.ends_with("file_macro.rs"));
}
