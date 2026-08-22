#!/usr/bin/env rust

//! A fold keeps its init's type through a `map` in the chain, so the default after a
//! `checked_mul` is a zero and not an empty string.

fn main() {
    let seed: u8 = 145;
    let item: u8 = 254;
    let last = vec![65534u16]
        .into_iter()
        .map(|_x: u16| item)
        .fold(seed, |_acc, x| x);
    println!("folded: {last}");

    // 254 * 209 overflows u8, so the default must be a u8 zero
    println!(
        "overflow default: {:?}",
        last.checked_mul(209).unwrap_or_default()
    );
    println!("checked product: {:?}", last.checked_mul(209));

    // in range
    println!("in range: {:?}", last.checked_add(1).unwrap_or_default());
}
