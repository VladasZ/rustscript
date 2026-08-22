#!/usr/bin/env rust

//! Declared numeric types are real widths at runtime. A helper declared `-> u8` returns a u8, so
//! a checked multiply past the u8 bound is None.

fn opaque(value: u8) -> u8 {
    value
}

fn add_ten(v: u8) -> u8 {
    v.wrapping_add(10)
}

fn early(v: u8) -> u8 {
    if v > 100 {
        return v;
    }
    v.wrapping_mul(2)
}

fn main() {
    // 254 * 209 overflows u8
    println!("checked product: {:?}", opaque(254).checked_mul(209));
    println!(
        "overflow default: {:?}",
        opaque(254).checked_mul(209).unwrap_or_default()
    );

    // the parameter width inside the body
    println!("wrapped in body: {}", add_ten(250));

    // an early return
    println!("early return: {:?}", early(200).checked_add(100));
    println!("tail return: {:?}", early(50).checked_add(100));

    // an annotated closure
    let keep = |x: u8| -> u8 { x };
    println!("closure width: {:?}", keep(250).checked_add(50));

    // wrapping arithmetic on 2 returned values
    println!("wrapping sum: {}", opaque(200).wrapping_add(opaque(100)));
}
