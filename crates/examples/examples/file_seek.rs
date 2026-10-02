#!/usr/bin/env rust

use std::fs::{File, remove_file};
use std::io::{Read, Seek, SeekFrom, Write};

fn main() -> anyhow::Result<()> {
    let path = std::env::temp_dir().join("rustscript_file_seek.txt");
    let mut created = File::create(&path)?;
    created.write_all(b"0123456789abcdef")?;
    created.flush()?;

    let mut file = File::open(&path)?;
    let pos = file.seek(SeekFrom::Start(10))?;
    let mut rest = String::new();
    file.read_to_string(&mut rest)?;
    println!("start literal: pos {pos} rest {rest}");

    let offset: u64 = 4;
    let pos = file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    println!("start u64: pos {pos} read {}", bytes.len());

    let back: i64 = -3;
    let pos = file.seek(SeekFrom::End(back))?;
    let mut tail = String::new();
    file.read_to_string(&mut tail)?;
    println!("end: pos {pos} tail {tail}");

    file.seek(SeekFrom::Start(2))?;
    let pos = file.seek(SeekFrom::Current(5))?;
    let mut after = String::new();
    file.read_to_string(&mut after)?;
    println!("current: pos {pos} after {after}");

    remove_file(&path)?;
    Ok(())
}
