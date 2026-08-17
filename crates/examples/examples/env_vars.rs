#!/usr/bin/env rust

// Set, read, enumerate, and remove environment variables. A missing
// variable answers the structured `VarError::NotPresent`.

use std::env;
use std::env::VarError;

fn main() {
    unsafe {
        env::set_var("RUSTSCRIPT_DEMO", "on");
    }
    println!("get: {:?}", env::var("RUSTSCRIPT_DEMO").ok());

    let present = env::vars().any(|(k, _)| k == "RUSTSCRIPT_DEMO");
    println!("found in vars(): {present}");

    unsafe {
        env::remove_var("RUSTSCRIPT_DEMO");
    }
    println!("after remove: {:?}", env::var("RUSTSCRIPT_DEMO").ok());

    let missing = env::var("RUSTSCRIPT_DEMO");
    println!("error: {missing:?}");
    match missing {
        Err(VarError::NotPresent) => println!("not present"),
        other => println!("unexpected: {other:?}"),
    }
    if let Err(e) = env::var("RUSTSCRIPT_DEMO") {
        println!("display: {e}");
    }
}
