#!/usr/bin/env rust

// A failing `lines()` item gives a structured `std::io::Error`.

use std::fs::File;
use std::io::{BufRead, BufReader, ErrorKind, Write};

fn main() {
    let dir = std::env::temp_dir().join("rustscript-lines-io-error");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mixed.txt");
    let mut f = File::create(&path).unwrap();
    f.write_all(b"good line\n\xff\xfe broken\nafter\n").unwrap();
    drop(f);

    let reader = BufReader::new(File::open(&path).unwrap());
    for line in reader.lines() {
        match line {
            Ok(text) => println!("ok: {text}"),
            Err(e) => {
                println!("kind: {:?}", e.kind());
                println!("is invalid data: {}", e.kind() == ErrorKind::InvalidData);
            }
        }
    }
    std::fs::remove_file(&path).unwrap();
}
