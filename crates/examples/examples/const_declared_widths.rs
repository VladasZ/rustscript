#!/usr/bin/env rust

//! A declared const type is a real width at runtime. Without it a `const N: u16` runs as a plain
//! untagged int, so it refuses to meet a real u16 in one operation and its own bound is never
//! enforced.

const LABEL_WIDTH: u16 = 10;
const GAP: u16 = 2;
const BOX_WIDTH: u16 = LABEL_WIDTH + GAP + 3;

const SMALL: u8 = 200;
const WIDE: u32 = 70_000;
const SIGNED: i8 = -100;
const COUNT: usize = 7;

static STATIC_WIDTH: u16 = 65_535;

struct Screen;

impl Screen {
    const ROWS: u8 = 250;
}

fn main() {
    // a bare literal on either side of a const
    println!("literal first: {}", 3 + LABEL_WIDTH);
    println!("literal second: {}", LABEL_WIDTH + 3);

    // a const meeting a value that already carries that width
    let width: u16 = 4;
    println!("const and let: {}", LABEL_WIDTH + width);
    println!("const from consts: {BOX_WIDTH}");

    // the declared bound is enforced
    println!("u16 bound: {:?}", STATIC_WIDTH.checked_add(1));
    println!("u8 bound: {:?}", SMALL.checked_add(100));
    println!("i8 bound: {:?}", SIGNED.checked_sub(100));
    println!("u32 room: {:?}", WIDE.checked_add(100));

    // width sensitive methods read the declared type
    println!("u16 zeros: {}", LABEL_WIDTH.leading_zeros());
    println!("u8 zeros: {}", SMALL.leading_zeros());
    println!("u32 zeros: {}", WIDE.leading_zeros());

    // an impl const and a usize const
    println!("assoc bound: {:?}", Screen::ROWS.checked_add(10));
    println!("usize sum: {}", COUNT + 1);
}
