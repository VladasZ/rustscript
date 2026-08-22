#!/usr/bin/env rust

//! A narrow `fold` init keeps its width. The dispatch once flattened it to
//! i64, so `leading_zeros` counted 64 bits instead of 32.

fn opaque(v: i32) -> i32 {
    v
}

fn main() {
    let init: i32 = opaque(502_028_173);

    // Empty, so the init comes back untouched.
    let empty = Vec::<u16>::new()
        .into_iter()
        .map(|_x: u16| opaque(0))
        .fold(init, |acc, _x| acc);
    println!("{}", empty.leading_zeros());

    // With items.
    let walked = vec![1u16, 2u16].into_iter().fold(init, |acc, _x| acc);
    println!("{}", walked.leading_zeros());

    // An accumulator the closure advances.
    let start: u8 = 200;
    let summed = vec![1u8, 2u8, 3u8]
        .into_iter()
        .fold(start, u8::saturating_add);
    println!("{}", summed.leading_zeros());
}
