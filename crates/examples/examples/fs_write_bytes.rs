#!/usr/bin/env rust


use std::fs;

use tempfile::tempdir;

fn main() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let file = dir.path().join("bytes.txt");

    let text = String::from("abc");
    fs::write(&file, text.as_bytes())?;
    println!("as_bytes: {}", fs::read_to_string(&file)?);

    let owned: Vec<u8> = vec![104, 105];
    fs::write(&file, &owned)?;
    println!("vec ref: {}", fs::read_to_string(&file)?);

    fs::write(&file, &owned[1..])?;
    println!("slice: {}", fs::read_to_string(&file)?);

    fs::write(&file, owned)?;
    println!("vec: {}", fs::read_to_string(&file)?);

    fs::write(&file, b"literal")?;
    println!("byte literal: {}", fs::read_to_string(&file)?);

    fs::write(&file, [111, 107])?;
    println!("array: {}", fs::read_to_string(&file)?);

    fs::write(&file, [0, 159, 255])?;
    println!("raw: {:?}", fs::read(&file)?);

    fs::write(&file, &text)?;
    println!("string ref: {}", fs::read_to_string(&file)?);

    fs::write(&file, text)?;
    println!("string: {}", fs::read_to_string(&file)?);

    Ok(())
}
