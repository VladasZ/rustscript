#!/usr/bin/env rust

//! A fold's result keeps its init's type through every step, and a later
//! `checked_*` call answers an Option of that same width. The campaign found
//! the width hint getting lost when a `map` sat in the chain, so the None
//! from an overflowing `checked_mul` defaulted to an empty string instead
//! of a zero of the folded integer type.

fn main() {
    let seed: u8 = 145;
    let item: u8 = 254;
    let last = vec![65534u16]
        .into_iter()
        .map(|_x: u16| item)
        .fold(seed, |_acc, x| x);
    println!("folded: {last}");

    // 254 * 209 overflows u8, so the checked product is None and the
    // default must be a u8 zero.
    println!(
        "overflow default: {:?}",
        last.checked_mul(209).unwrap_or_default()
    );
    println!("checked product: {:?}", last.checked_mul(209));

    // In range, the width rides along untouched.
    println!("in range: {:?}", last.checked_add(1).unwrap_or_default());
}
