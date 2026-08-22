#!/usr/bin/env rust

//! `rotate_left` and `rotate_right` on an integer return a value, they don't mutate.

fn opaque_i16(v: i16) -> i16 {
    v
}

fn opaque_u32(v: u32) -> u32 {
    v
}

fn main() {
    let n: u8 = 0b1000_0001;
    println!("rotated: {} {}", n.rotate_left(1), n.rotate_right(1));
    println!("unchanged: {n}");

    // the receiver keeps its value through a capturing closure too
    let mut folded: i16 = opaque_i16(32_766);
    folded = Vec::<u8>::new()
        .into_iter()
        .map(|_| folded.rotate_right(opaque_u32(9)))
        .fold(opaque_i16(0), |_acc, _item| opaque_i16(0));
    println!("folded: {folded}");

    let mut direct: i16 = opaque_i16(9);
    direct = direct.rotate_left(opaque_u32(1));
    println!("assigned: {direct}");
}
