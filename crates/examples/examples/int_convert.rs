#!/usr/bin/env rust

// Widening `from` and fallible `try_from` between integer types, plus the
// `bool` and `float` conversions clippy suggests in place of `as` casts.

fn main() {
    let small: i32 = 42;
    let wide: i64 = i64::from(small);
    let as_float: f64 = f64::from(small);
    println!("from: {wide} {as_float}");

    println!("bool: {} {}", usize::from(true), i64::from(false));

    for n in [10i64, 200, 300, -1] {
        match u8::try_from(n) {
            Ok(v) => println!("u8::try_from({n}) = {v}"),
            Err(_) => println!("u8::try_from({n}) = overflow"),
        }
    }

    let big: i64 = 5_000_000_000;
    println!("i32 fits: {}", i32::try_from(big).is_ok());
    println!("i32 clamp: {}", i32::try_from(big).unwrap_or(-1));
    println!("usize ok: {}", usize::try_from(7i64).unwrap_or(0));
}
