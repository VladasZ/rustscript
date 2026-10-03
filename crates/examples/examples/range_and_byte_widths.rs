#!/usr/bin/env rust

//! A range yields items of the width of its bounds, and the bytes of a string are `u8`.

fn main() {
    for i in 0u8..3 {
        println!("{} {}", !i, i.leading_zeros());
    }
    for i in (0..3usize).rev() {
        println!("{}", !i);
    }
    let mut items = vec![7u32, 8];
    items.push(9);
    for i in 0..items.len() {
        println!("{} {}", i.is_power_of_two(), !i);
    }
    let last: u16 = 2;
    for i in 1..=last {
        println!("{}", i.wrapping_sub(2));
    }
    let flipped: Vec<u8> = (0u8..3).map(|x| !x).collect();
    println!("{flipped:?}");
    let indexes: Vec<usize> = (0..items.len()).collect();
    println!("{}", !indexes[1]);
    println!("{}", (1i8..4).map(|x| x.wrapping_mul(100)).sum::<i8>());
    let plain: i64 = (0..4).sum();
    println!("{plain}");

    for b in "ab".bytes() {
        println!("{} {}", !b, b.wrapping_add(200));
    }
    println!("{}", !"ab".as_bytes()[0]);
    let owned = String::from("az").into_bytes();
    println!("{} {}", owned[1].count_ones(), !owned[1]);
    let literal = b"hi";
    println!("{}", !literal[0]);
    let wide: Vec<u16> = "a\u{10000}".encode_utf16().collect();
    println!("{} {}", wide.len(), !wide[0]);
    println!("{:?}", 258u16.to_be_bytes().map(|b| !b));
}
