#!/usr/bin/env rust

// The suite gives no stdin, so the stdin take reads 0 bytes in both runs.

use std::fs::{File, remove_file};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write, stdin};

fn main() -> anyhow::Result<()> {
    let path = std::env::temp_dir().join("rustscript_read_take.txt");
    File::create(&path)?.write_all(b"alpha\nbeta\ngamma\ndelta\n")?;

    let mut text = String::new();
    let n = File::open(&path)?.take(8).read_to_string(&mut text)?;
    println!("take 8 read {n} {text:?}");

    let mut bytes = Vec::new();
    let n = File::open(&path)?.take(100).read_to_end(&mut bytes)?;
    println!("take past the end read {n}");

    let mut reader = BufReader::new(File::open(&path)?);
    let mut line = String::new();
    let n = reader.by_ref().take(3).read_line(&mut line)?;
    let mut rest = String::new();
    reader.read_line(&mut rest)?;
    println!("by_ref take 3 line {n} {line:?}, then {rest:?}");

    let mut limited = File::open(&path)?.take(12);
    let mut head = [0u8; 4];
    limited.read_exact(&mut head)?;
    println!(
        "read_exact {:?}, limit now {}",
        String::from_utf8_lossy(&head),
        limited.limit()
    );
    let mut short = [0u8; 10];
    let err = limited.read_exact(&mut short).unwrap_err();
    println!(
        "read_exact past the limit {}",
        err.kind() == ErrorKind::UnexpectedEof
    );
    limited.set_limit(6);
    let mut more = String::new();
    limited.read_to_string(&mut more)?;
    println!("after set_limit {more:?}, limit {}", limited.limit());
    let mut file = limited.into_inner();
    let mut tail = String::new();
    file.read_to_string(&mut tail)?;
    println!("into_inner rest {tail:?}");

    let lines: Vec<String> = BufReader::new(File::open(&path)?)
        .take(14)
        .lines()
        .collect::<Result<_, _>>()?;
    println!("lines of take 14 {lines:?}");

    let mut until = Vec::new();
    let n = BufReader::new(File::open(&path)?)
        .take(4)
        .read_until(b'\n', &mut until)?;
    println!(
        "read_until in take 4 read {n} {:?}",
        String::from_utf8_lossy(&until)
    );

    let mut nested = String::new();
    File::open(&path)?
        .take(10)
        .take(3)
        .read_to_string(&mut nested)?;
    println!("nested take {nested:?}");

    let mut raw = String::new();
    let n = stdin().take(2_097_153).read_to_string(&mut raw)?;
    println!("stdin take read {n} {raw:?}");

    remove_file(&path)?;
    Ok(())
}
