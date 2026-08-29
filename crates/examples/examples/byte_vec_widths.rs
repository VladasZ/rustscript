#!/usr/bin/env rust

// A vec the script declares as `Vec<u8>` carries the byte width, while the same bytes handed back
// by a bridge are plain ints. Every call that takes bytes has to read both forms.

use base64::prelude::*;
use hex::{decode, encode};
use sha2::{Digest, Sha256};

fn main() -> anyhow::Result<()> {
    let declared: Vec<u8> = vec![104, 105, 33];
    let from_bridge = decode(encode(&declared))?;

    for bytes in [&declared, &from_bridge] {
        println!("utf8: {}", String::from_utf8(bytes.clone())?);
        println!("lossy: {}", String::from_utf8_lossy(bytes));
        println!("hex: {}", encode(bytes));
        println!("base64: {}", BASE64_STANDARD.encode(bytes));
        println!("sha256: {}", encode(Sha256::digest(bytes)));
    }

    let mut mixed: Vec<u8> = Vec::new();
    mixed.push(0);
    mixed.push(255);
    mixed.extend(declared.iter().copied());
    println!("mixed hex: {}", encode(&mixed));
    println!("mixed lossy: {}", String::from_utf8_lossy(&mixed));
    Ok(())
}
