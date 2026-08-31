#!/usr/bin/env rust

fn main() {
    println!("{}", 0u8.next_power_of_two());
    println!("{}", 1u8.next_power_of_two());
    println!("{}", 5u8.next_power_of_two());
    println!("{}", 128u8.next_power_of_two());
    println!("{}", 300u16.next_power_of_two());
    println!("{}", 70_000u32.next_power_of_two());
    println!("{}", 5_000_000_000u64.next_power_of_two());
    println!("{}", 100usize.next_power_of_two());
    println!("{}", (1u128 << 100).next_power_of_two());

    println!("{:?}", 5u32.checked_next_power_of_two());
    println!("{:?}", 0u16.checked_next_power_of_two());
    println!("{:?}", 200u8.checked_next_power_of_two());
    println!("{:?}", u8::MAX.checked_next_power_of_two());
    println!("{:?}", u64::MAX.checked_next_power_of_two());
    println!("{:?}", usize::MAX.checked_next_power_of_two());
    println!("{:?}", u128::MAX.checked_next_power_of_two());
    println!("{:?}", ((1u128 << 100) + 1).checked_next_power_of_two());

    println!("{}", 64u32.is_power_of_two());
    println!("{}", 65u32.is_power_of_two());
    println!("{}", 0u8.is_power_of_two());
    println!("{}", (1u128 << 127).is_power_of_two());

    let sizes: Vec<u32> = vec![3, 8, 17];
    let rounded: Vec<u32> = sizes.iter().map(|s| s.next_power_of_two()).collect();
    println!("{rounded:?}");
    let widths: Vec<u8> = vec![3, 200];
    let checked: Vec<Option<u8>> = widths
        .iter()
        .map(|s| s.checked_next_power_of_two())
        .collect();
    println!("{checked:?}");
}
