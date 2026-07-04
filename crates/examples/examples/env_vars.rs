#!/usr/bin/env rust

// Set, read, enumerate, and remove environment variables.

use std::env;

fn main() {
    unsafe {
        env::set_var("RUSTSCRIPT_DEMO", "on");
    }
    println!("get: {:?}", env::var("RUSTSCRIPT_DEMO").ok());

    let present = env::vars().any(|(k, _)| k == "RUSTSCRIPT_DEMO");
    println!("found in vars(): {}", present);

    unsafe {
        env::remove_var("RUSTSCRIPT_DEMO");
    }
    println!("after remove: {:?}", env::var("RUSTSCRIPT_DEMO").ok());
}
