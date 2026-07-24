#!/usr/bin/env rust

// Integers carry their real width at runtime and floats exist at both
// precisions, so arithmetic, casts, shifts, and printing agree with compiled
// Rust, including the full u64 and usize range past i64::MAX.

fn opaque(x: i64) -> i64 {
    x
}

fn opaque_float(x: f64) -> f64 {
    x
}

fn main() {
    let small: u8 = 200;
    let suffixed = 55u8;
    println!("u8 sum: {}", small + suffixed);

    let big: u64 = 18446744073709551615;
    println!("u64 max: {big}");
    println!("u64 half: {}", big / 2);
    let big_size = 18446744073709551614usize;
    println!("usize: {big_size}");
    println!("u64 vs literal: {}", big > 9223372036854775807);

    // Narrowing casts truncate, float to int casts saturate, NaN is zero.
    let wide = opaque(300);
    println!("as u8: {}", wide as u8);
    println!("as i8: {}", wide as i8);
    println!("negative as usize: {}", opaque(-1) as usize);
    println!("float as u8: {}", 300.9f64 as u8);
    println!("nan as i32: {}", opaque_float(f64::NAN) as i32);
    println!("neg as u16: {}", (-1.5f64) as u16);

    // Shifts discard bits like release Rust and keep the width.
    let byte = 255u8;
    println!("shl: {}", byte << 4);
    println!("shr: {}", byte >> 4);
    let signed = -128i8;
    println!("i8 shr: {}", signed >> 1);
    println!("negate: {}", -(opaque(-127) as i8));

    // f32 computes and prints at f32 precision, f64 at f64 precision.
    let a: f32 = 0.1;
    let b: f32 = 0.2;
    println!("f32 sum: {}", a + b);
    let x: f64 = 0.1;
    let y: f64 = 0.2;
    println!("f64 sum: {}", x + y);
    println!("f32 eps: {}", f32::EPSILON);
    println!("f32 debug: {:?}", 16777217.0f32);
    println!("i64 as f32: {}", opaque(16777217) as f32);
    println!("f32 as f64: {}", 0.1f32 as f64);
    println!("f32 inf: {} {}", f32::INFINITY, 1e30f32 * 1e30f32);

    // Radix specs print the two's complement image at the value's width.
    let neg = opaque(-1) as i8;
    println!("i8 hex: {neg:x}");
    println!("u64 hex: {big:x}");
    println!("u8 bin: {:b}", 200u8);

    println!("{} {} {} {}", u8::MAX, i8::MIN, u32::MAX, u64::MAX);
    println!("{} {}", i64::MIN, usize::MAX);
}
