#!/usr/bin/env rust

// Real 128-bit integers: full-range values, arithmetic with overflow
// checks, casts, comparisons, parsing, and formatting.

fn main() {
    let max: u128 = u128::MAX;
    println!("{max}");
    println!("{}", i128::MIN);
    println!("{}", i128::MAX);

    let big: u128 = 1u128 << 100;
    println!("{big}");
    println!("{}", big * 3);
    println!("{}", big + 1 > big);

    let signed: i128 = -170_141_183_460_469_231_731_687_303_715_884_105_727;
    println!("{}", signed - 1 == i128::MIN);

    let wide = u128::from(250u8) * 1_000_000_000_000_000_000_000u128;
    println!("{wide}");
    println!("{}", u64::try_from((1u128 << 90) >> 40).unwrap());
    println!("{}", u8::try_from(u128::MAX >> 121).unwrap());

    let parsed: u128 = "123456789012345678901234567890".parse().unwrap();
    println!("{parsed}");
    let bad = "not a number".parse::<i128>();
    println!("{}", bad.is_err());

    println!("{}", u128::MAX / 7);
    let negative: i128 = i128::MIN + 1;
    println!("{}", negative % 7);
    println!("{:?}", 5i128.checked_add(3));
    let halves = (u128::MAX / 2, u128::MAX % 2);
    println!("{halves:?}");
}
