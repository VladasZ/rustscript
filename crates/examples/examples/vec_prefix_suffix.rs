#!/usr/bin/env rust

// `starts_with` and `ends_with` on a vec, against both a declared `Vec<u8>`
// and plain bridge bytes, the two byte forms every call has to read.

use base64::prelude::*;

fn main() -> anyhow::Result<()> {
    let declared: Vec<u8> = b"untrusted comment: hello".to_vec();
    let from_bridge = BASE64_STANDARD.decode(BASE64_STANDARD.encode(&declared))?;

    for bytes in [&declared, &from_bridge] {
        println!("prefix: {}", bytes.starts_with(b"untrusted comment:"));
        println!("wrong prefix: {}", bytes.starts_with(b"trusted"));
        println!("suffix: {}", bytes.ends_with(b"hello"));
        println!("wrong suffix: {}", bytes.ends_with(b"world"));
        println!("longer than vec: {}", bytes.starts_with(&[0; 99]));
        println!("empty needle: {}", bytes.starts_with(b""));
    }

    let numbers = vec![1, 2, 3, 4];
    println!("ints prefix: {}", numbers.starts_with(&[1, 2]));
    println!("ints suffix: {}", numbers.ends_with(&[3, 4]));
    println!("ints wrong: {}", numbers.ends_with(&[1, 4]));
    println!("self: {}", numbers.starts_with(&numbers));
    Ok(())
}
