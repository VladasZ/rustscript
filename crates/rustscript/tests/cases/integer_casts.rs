#!/usr/bin/env rust

// Narrowing `as` casts must truncate, not keep the full i64 value.

fn main() {
    let samples: Vec<i64> = vec![
        0,
        255,
        256,
        300,
        -1,
        -128,
        70000,
        -70000,
        2_147_483_648,
        -2_147_483_649,
    ];
    for value in samples {
        println!(
            "{value}: u8={} i8={} u16={} i16={} u32={} i32={}",
            value as u8, value as i8, value as u16, value as i16, value as u32, value as i32
        );
    }
}
