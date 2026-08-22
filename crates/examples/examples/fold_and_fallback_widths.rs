#!/usr/bin/env rust

//! A `fold` has its init's type and `unwrap_or` types its fallback from the payload. Neither
//! widens to i64.

fn opaque_i32(v: i32) -> i32 {
    v
}

fn opaque_i16(v: i16) -> i16 {
    v
}

fn opaque_i64(v: i64) -> i64 {
    v
}

fn opaque_u8(v: u8) -> u8 {
    v
}

fn opaque_f64(v: f64) -> f64 {
    v
}

fn opaque_bool(v: bool) -> bool {
    v
}

fn narrow_of(v: i32) -> Option<i32> {
    if opaque_bool(false) { Some(v) } else { None }
}

fn wide_of(v: i64) -> Option<i64> {
    if opaque_bool(false) { Some(v) } else { None }
}

fn main() {
    // bare literals in the body, `i32` from the init
    let folded = Vec::<i64>::new().into_iter().map(|_| opaque_i32(2)).fold(
        opaque_i32(950_127_717),
        |_acc, _item| {
            if opaque_bool(true) { 1 } else { 2 }
        },
    );
    println!("folded: {:#x}", !folded);

    // with items
    let summed = vec![opaque_i64(1), opaque_i64(2)]
        .into_iter()
        .fold(opaque_i16(0), |acc, _item| acc);
    println!("summed: {summed:#x}");

    println!(
        "narrow: {:+4x}",
        narrow_of(1).unwrap_or(if opaque_bool(true) { -2_147_483_648 } else { 0 })
    );
    println!(
        "wide: {:#x}",
        wide_of(1).unwrap_or(if opaque_bool(true) { -2 } else { 0 })
    );

    // the fold type reaches a default at the end of the chain
    let scaled = Vec::<i8>::new()
        .into_iter()
        .map(|_| opaque_f64(0.0))
        .fold(opaque_f64(0.0), |acc, _item| acc);
    let row = [(opaque_u8(127), Some(scaled))]
        .get(5)
        .copied()
        .unwrap_or_default();
    println!("row: {row:?}");
}
