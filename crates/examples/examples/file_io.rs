#!/usr/bin/env rust

// Open a file with the low level API, write, seek back to the start, and read
// it again through a buffered reader.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};

fn main() -> anyhow::Result<()> {
    let path = std::env::temp_dir().join("rustscript_file_io.txt");
    let path = path.to_string_lossy().to_string();

    let mut f = File::create(&path)?;
    f.write_all(b"first line\n")?;
    f.write_all(b"second line\n")?;
    f.flush()?;

    f.seek(SeekFrom::Start(0))?;
    let mut contents = String::new();
    let mut reader = File::open(&path)?;
    reader.read_to_string(&mut contents)?;

    println!("bytes written: {}", contents.len());
    println!("lines: {}", contents.lines().count());
    std::fs::remove_file(&path)?;
    Ok(())
}
