#!/usr/bin/env rust

// Create a temporary directory, write into it, read it back. The directory is
// cleaned up when the handle is dropped.

use std::fs;

fn main() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("note.txt");
    fs::write(&file, "scratch data")?;

    let read_back = fs::read_to_string(&file)?;
    println!("roundtrip ok: {}", read_back == "scratch data");
    println!("dir existed: {}", dir.path().is_dir());
    Ok(())
}
