#!/usr/bin/env rust

//! An unsigned parse rejects a minus sign as an invalid digit before any
//! range check, so "-0" is an error even though zero fits. The campaign
//! found the interpreter parsing through a wide signed integer, which
//! accepted "-0" by range and turned a validation branch around.

fn main() {
    // Negative zero formats with its sign, which is where a "-0" string
    // comes from in a real program.
    let text = (-0.0f32).to_string();
    println!("text:           {text:?}");
    println!("u64 minus zero: {}", text.parse::<u64>().is_err());
    println!("u8 minus zero:  {}", "-0".parse::<u8>().is_err());
    println!("i64 minus zero: {:?}", "-0".parse::<i64>());
    println!("u64 plus seven: {:?}", "+7".parse::<u64>());
}
