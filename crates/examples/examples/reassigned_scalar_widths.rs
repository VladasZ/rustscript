#!/usr/bin/env rust

//! A reassigned local keeps its declared width, so `{:b}` of an `i32` prints
//! 32 digits.

fn opaque_bool(v: bool) -> bool {
    v
}

fn main() {
    let mut small: i32 = 1;
    println!("{small:b}");
    small = -2_147_483_647;
    println!("{small:b} {small}");

    let mut byte: i8 = 1;
    println!("{byte:b}");
    byte = -1;
    println!("{byte:b} {byte:x} {byte:o}");

    let mut wide: i16 = 1;
    println!("{wide:x}");
    wide = -2;
    println!("{wide:x} {wide:b}");

    // Through a branch.
    let mut picked: i32 = 1;
    println!("{picked:b}");
    picked = if opaque_bool(true) { -2_147_483_647 } else { 0 };
    println!("{picked:*^9b} {picked:?}");

    let mut byte_u: u8 = 1;
    println!("{byte_u:b}");
    byte_u = 255;
    println!("{byte_u:b} {byte_u:x}");
}
