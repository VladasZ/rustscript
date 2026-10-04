#!/usr/bin/env rust

//! A call by path has a known result type. So an integer built from bytes keeps its width, and
//! a function imported by name answers like its full path does.

use std::fs::{read_to_string, write};
use std::process::id;

fn main() {
    let small = u32::from_le_bytes([1, 0, 0, 0]);
    println!("{} {}", !small, small.leading_zeros());
    let signed = i16::from_be_bytes([255, 0]);
    println!("{signed} {:?}", signed.checked_mul(200));
    let byte = u8::from_ne_bytes([200]);
    println!("{:?}", byte.checked_add(100));
    let float = f32::from_le_bytes([0, 0, 128, 63]);
    println!("{float} {}", float / 3.0);

    // the payload type decides what the default is
    let none: Option<u32> = None;
    let fallback = none
        .map(|_| u32::from_le_bytes([1, 0, 0, 0]))
        .unwrap_or_default();
    println!("{}", !fallback);

    // a method named by path
    println!("{}", u8::leading_zeros(1));
    println!("{:?}", i8::checked_add(100, 100));
    println!("{}", u16::wrapping_sub(1, 2));

    let dir = std::env::temp_dir().join(format!("rustscript_typed_path_calls_{}", id() > 0));
    std::fs::create_dir_all(&dir).expect("create dir");
    let file = dir.join("note.txt");
    write(&file, "hello").expect("write");
    let text = read_to_string(&file).unwrap_or_default();
    println!("{text} {}", text.len());
    let missing = read_to_string(dir.join("none.txt")).unwrap_or_default();
    println!("[{missing}] {}", missing.is_empty());
    std::fs::remove_dir_all(&dir).expect("remove dir");
}
