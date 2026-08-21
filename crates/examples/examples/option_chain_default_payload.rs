#!/usr/bin/env rust

//! `unwrap_or_default()` finds its payload type through an `Option` chain.
//! `and` answers its argument's payload, not the receiver's, and `or` and
//! `xor` keep the payload both sides share.

fn opaque_u8(v: u8) -> u8 {
    v
}

fn opaque_i16(v: i16) -> i16 {
    v
}

fn main() {
    let plain = opaque_u8(255)
        .checked_div(opaque_u8(254))
        .unwrap_or_default();
    println!("plain: {plain:?}");

    let through_and = opaque_u8(255)
        .checked_div(opaque_u8(254))
        .and(None::<u8>)
        .unwrap_or_default();
    println!("through_and: {through_and:?}");

    // `and` takes the argument's type, which need not match the receiver's.
    let widened = opaque_u8(1)
        .checked_add(opaque_u8(1))
        .and(None::<i64>)
        .unwrap_or_default();
    println!("widened: {widened:?}");

    let through_or = opaque_i16(5)
        .checked_add(opaque_i16(1))
        .or(None)
        .unwrap_or_default();
    println!("through_or: {through_or:?}");

    let either_side = opaque_u8(1)
        .checked_div(opaque_u8(0))
        .xor(None::<u8>)
        .unwrap_or_default();
    println!("either_side: {either_side:?}");
}
