#!/usr/bin/env rust

//! `fold` passes its initial value through to the closure and the result, so
//! a narrow integer init keeps its width on both the lazy iterator path and
//! the eager vec path. The campaign found the dispatch flattening the init to
//! a plain i64, so `leading_zeros` counted 64 bits instead of 32.

fn opaque(v: i32) -> i32 {
    v
}

fn main() {
    let init: i32 = opaque(502_028_173);

    // The lazy iterator path, empty so the init comes back untouched.
    let empty = Vec::<u16>::new()
        .into_iter()
        .map(|_x: u16| opaque(0))
        .fold(init, |acc, _x| acc);
    println!("{}", empty.leading_zeros());

    // The lazy path again with items, so the accumulator rides the closure.
    let walked = vec![1u16, 2u16].into_iter().fold(init, |acc, _x| acc);
    println!("{}", walked.leading_zeros());

    // A narrow accumulator that the closure actually advances.
    let start: u8 = 200;
    let summed = vec![1u8, 2u8, 3u8]
        .into_iter()
        .fold(start, u8::saturating_add);
    println!("{}", summed.leading_zeros());
}
