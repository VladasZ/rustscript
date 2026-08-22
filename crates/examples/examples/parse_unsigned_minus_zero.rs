#!/usr/bin/env rust

//! An unsigned parse rejects "-0" even though zero fits.

fn main() {
    // negative zero formats with its sign, that is where "-0" comes from
    let text = (-0.0f32).to_string();
    println!("text:           {text:?}");
    println!("u64 minus zero: {}", text.parse::<u64>().is_err());
    println!("u8 minus zero:  {}", "-0".parse::<u8>().is_err());
    println!("i64 minus zero: {:?}", "-0".parse::<i64>());
    println!("u64 plus seven: {:?}", "+7".parse::<u64>());
}
