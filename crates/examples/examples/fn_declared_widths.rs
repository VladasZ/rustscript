#!/usr/bin/env rust

//! A function's declared numeric types are real widths at runtime. The
//! parameter retags what the caller passed, the return type retags what the
//! body produced, and closures with annotations follow the same rule. Before
//! this, a helper declared `-> u8` answered an untagged wide value, so a
//! checked multiply that must overflow at the u8 bound quietly computed wide
//! and answered Some.

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
    // 254 * 209 overflows u8, so the checked product is None and its
    // default is a u8 zero.
    println!("checked product: {:?}", opaque(254).checked_mul(209));
    println!(
        "overflow default: {:?}",
        opaque(254).checked_mul(209).unwrap_or_default()
    );

    // The parameter width holds inside the body.
    println!("wrapped in body: {}", add_ten(250));

    // An early return is retagged the same as the tail.
    println!("early return: {:?}", early(200).checked_add(100));
    println!("tail return: {:?}", early(50).checked_add(100));

    // A closure with annotations follows the same rule.
    let keep = |x: u8| -> u8 { x };
    println!("closure width: {:?}", keep(250).checked_add(50));

    // Wrapping arithmetic on two returned values stays in u8.
    println!("wrapping sum: {}", opaque(200).wrapping_add(opaque(100)));
}
