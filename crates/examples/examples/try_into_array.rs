#!/usr/bin/env rust

use std::convert::TryInto;

fn sum(word: [u8; 4]) -> u32 {
    word.iter().map(|b| u32::from(*b)).sum()
}

fn parse_size(bytes: &[u8]) -> Result<u32, std::array::TryFromSliceError> {
    let word: [u8; 4] = bytes.try_into()?;
    Ok(u32::from_be_bytes(word))
}

fn size_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

// Reads the width and height out of a png header, the way the app-icon picker does.
fn main() {
    let bytes: Vec<u8> = vec![0, 0, 4, 0, 9];
    let word: [u8; 4] = bytes[0..4].try_into().unwrap();
    println!("{}", u32::from_be_bytes(word));
    println!("{word:?}");

    let header: Vec<u8> = vec![0, 0, 1, 0, 0, 0, 0, 200, 7];
    let width = u32::from_be_bytes(header[0..4].try_into().unwrap());
    let height = u32::from_be_bytes(header[4..8].try_into().expect("4 bytes"));
    println!("{width}x{height}");
    println!(
        "le: {}",
        u16::from_le_bytes(header[2..4].try_into().unwrap())
    );
    println!("sum: {}", sum(header[4..8].try_into().unwrap()));
    println!(
        "at 4: {:?}, at 7: {:?}",
        size_at(&header, 4),
        size_at(&header, 7)
    );
    println!("get: {:?} {:?}", header.get(..=1), header.get(8..20));

    println!("size: {:?}", parse_size(&header[..4]));
    match parse_size(&header[..3]) {
        Ok(n) => println!("unexpected {n}"),
        Err(e) => println!("short: {e} / {e:?}"),
    }
    let short: Result<[u8; 2], _> = bytes[..3].try_into();
    println!("short result: {short:?}");

    let big: i64 = 300;
    let small: Result<u8, _> = big.try_into();
    println!("u8 from 300: {small:?}");
    let fits: u16 = big.try_into().unwrap();
    println!("u16 from 300: {fits}");
    let negative: Result<u32, _> = (-1i32).try_into();
    match negative {
        Ok(n) => println!("unexpected {n}"),
        Err(e) => println!("u32 from -1: {e} / {e:?}"),
    }
    let wide: i128 = 70_000;
    let back: i32 = wide.try_into().unwrap();
    println!("i32 from i128: {back}");
}
