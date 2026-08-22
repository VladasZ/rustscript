#!/usr/bin/env rust

//! Every line is chosen so a wrong byte order or a lost sign prints something different.

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<String>>()
        .join(" ")
}

fn main() {
    // these 2 lines are reverses of each other
    let word: u32 = 0x1234_5678;
    println!("u32 le: {}", hex(&word.to_le_bytes()));
    println!("u32 be: {}", hex(&word.to_be_bytes()));
    println!("u16 le: {}", hex(&0xabcdu16.to_le_bytes()));
    println!("u16 be: {}", hex(&0xabcdu16.to_be_bytes()));
    println!("u64 be: {}", hex(&1u64.to_be_bytes()));
    println!("u64 le: {}", hex(&1u64.to_le_bytes()));
    println!("u8  le: {}", hex(&200u8.to_le_bytes()));
    println!("usize: {}", 1usize.to_le_bytes().len());

    // printing which order native matches keeps the line stable on any host
    let native = word.to_ne_bytes();
    println!("u32 ne is le: {}", native == word.to_le_bytes());
    println!("u32 ne is be: {}", native == word.to_be_bytes());

    // the sign lives in the byte the order puts first
    println!("i16 -2 be: {}", hex(&(-2i16).to_be_bytes()));
    println!("i16 -2 le: {}", hex(&(-2i16).to_le_bytes()));
    println!("i32 -1 le: {}", hex(&(-1i32).to_le_bytes()));
    println!("i8 min be: {}", hex(&i8::MIN.to_be_bytes()));

    // all 4 results differ
    let raw = [0x78u8, 0x56, 0x34, 0x12];
    println!("u32 from le: {}", u32::from_le_bytes(raw));
    println!("u32 from be: {}", u32::from_be_bytes(raw));
    println!("i32 from le: {}", i32::from_le_bytes(raw));
    println!("i32 from be: {}", i32::from_be_bytes(raw));

    // the top bit set
    let high = [0xffu8, 0xff, 0xff, 0xff];
    println!("u32 all ones: {}", u32::from_le_bytes(high));
    println!("i32 all ones: {}", i32::from_le_bytes(high));
    println!("i16 high be: {}", i16::from_be_bytes([0x80, 0x00]));
    println!("u16 high be: {}", u16::from_be_bytes([0x80, 0x00]));
    println!("i8 high: {}", i8::from_le_bytes([0x80]));
    println!("u8 high: {}", u8::from_le_bytes([0x80]));
    println!(
        "u64 from be: {}",
        u64::from_be_bytes([0, 0, 0, 0, 0, 0, 1, 0])
    );
    println!("i64 from le: {}", i64::from_le_bytes([0xff; 8]));
    println!(
        "usize from le: {}",
        usize::from_le_bytes([2, 0, 0, 0, 0, 0, 0, 0])
    );
    println!("isize from be: {}", isize::from_be_bytes([0xff; 8]));

    // round trips
    for value in [0i32, 1i32, -1i32, i32::MIN, i32::MAX, -123_456i32] {
        let there = value.to_be_bytes();
        let back = i32::from_be_bytes(there);
        println!("round {value} -> {} -> {back}", hex(&there));
    }

    // the result keeps the width it was read as
    let small = u8::from_be_bytes([250]);
    println!("u8 saturating: {}", small.saturating_add(10));
    println!("u8 wrapping: {}", small.wrapping_add(10));
    println!("u8 checked: {:?}", small.checked_add(10));

    // a real BMP header
    let header: Vec<u8> = vec![
        0x42, 0x4d, 0x36, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x36, 0x00, 0x00, 0x00, 0x28,
        0x00, 0x00, 0x00, 0x80, 0x02, 0x00, 0x00, 0x9c, 0xff, 0xff, 0xff, 0x01, 0x00, 0x18, 0x00,
    ];
    let offset = u32::from_le_bytes([header[10], header[11], header[12], header[13]]);
    let width = i32::from_le_bytes([header[18], header[19], header[20], header[21]]);
    let height = i32::from_le_bytes([header[22], header[23], header[24], header[25]]);
    let depth = u16::from_le_bytes([header[28], header[29]]);
    println!("bmp data offset: {offset}");
    println!("bmp size: {width}x{height} at {depth} bpp");
    println!("bmp top down: {}", height < 0);
}
