#!/usr/bin/env rust

// The 128 bit method surface.

fn main() {
    let max: u128 = u128::MAX;
    println!("{:?}", max.checked_add(1));
    println!("{}", max.wrapping_add(1));
    println!("{}", max.saturating_add(5));
    println!("{}", max.count_ones());
    println!("{}", max.leading_zeros());
    println!("{}", 1u128.leading_zeros());
    println!("{:?}", 2u128.checked_pow(100));
    println!("{}", 2u128.pow(100));
    println!("{}", max.rotate_left(8) == max);
    println!("{}", 1u128.rotate_right(1) == 1u128 << 127);

    let signed: i128 = i128::MIN;
    println!("{}", signed.wrapping_sub(1));
    println!("{}", signed.count_ones());
    println!("{:?}", signed.checked_neg());
    println!("{}", (-5i128).abs());
    println!("{}", (-5i128).signum());
    println!("{}", (1i128 << 100).isqrt());

    let bytes = max.to_le_bytes();
    println!("{}", bytes.len());
    println!("{}", u128::from_le_bytes(bytes));
    println!("{}", i128::from_be_bytes((-2i128).to_be_bytes()));

    println!(
        "{:?}",
        u128::from_str_radix("ffffffffffffffffffffffffffffffff", 16)
    );
    println!(
        "{:?}",
        i128::from_str_radix("-80000000000000000000000000000000", 16)
    );

    println!("{max:x}");
    println!("{:X}", -1i128);
    println!("{:#x}", 1u128 << 100);
    println!("{:b}", 1u128 << 127);
    println!("{:#o}", u128::MAX);

    let shifted: u128 = 1 << 100;
    println!("{shifted}");
    let masked: u128 = (1 << 100) | 255;
    println!("{masked:x}");
    let summed: i128 = 170_141_183_460_469_231_731_687_303_715_884_105_727 - 1 + 1;
    println!("{summed}");
}
