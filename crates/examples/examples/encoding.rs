#!/usr/bin/env rust

// base64 and hex encoding, both ways.

use base64::prelude::*;

fn main() -> anyhow::Result<()> {
    let text = b"rustscript";

    let b64 = BASE64_STANDARD.encode(text);
    println!("base64: {}", b64);
    let back = BASE64_STANDARD.decode(&b64)?;
    println!("base64 roundtrip: {}", String::from_utf8_lossy(&back));

    let url = BASE64_URL_SAFE_NO_PAD.encode(text);
    println!("base64 url safe: {}", url);

    let hexed = hex::encode(text);
    println!("hex: {}", hexed);
    let unhex = hex::decode(&hexed)?;
    println!("hex roundtrip: {}", String::from_utf8_lossy(&unhex));
    Ok(())
}
