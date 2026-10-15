#!/usr/bin/env rust

//! `std::str::from_utf8` and `String::from_utf8` check the bytes. Bytes that are not UTF-8 are an
//! `Err` that prints like the real error, the way a log reader rejects a broken record.

use std::str::from_utf8;

use anyhow::Context;

fn records(data: &[u8]) -> anyhow::Result<Vec<String>> {
    let text = from_utf8(data).context("record is not UTF-8")?;
    Ok(text
        .lines()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .collect())
}

fn main() {
    let good = b"one\n\ntwo\n".to_vec();
    println!("{:?}", records(&good).unwrap());

    let bad = vec![b'o', b'k', 0xff, b'!'];
    match records(&bad) {
        Ok(lines) => println!("unexpected {lines:?}"),
        Err(e) => println!("{e:#}"),
    }

    match std::str::from_utf8(&bad) {
        Ok(text) => println!("unexpected {text}"),
        Err(e) => println!("str: {e} / {e:?}"),
    }
    match String::from_utf8(bad) {
        Ok(text) => println!("unexpected {text}"),
        Err(e) => println!("string: {e}"),
    }
    println!("{:?}", String::from_utf8(vec![0xc3, 0xa9]).unwrap());
    println!("{:?}", from_utf8("plain".as_bytes()));
}
