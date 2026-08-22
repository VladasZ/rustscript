#!/usr/bin/env rust


use std::fs;

use tempfile::tempdir;

fn main() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let file = dir.path().join("note.txt");
    fs::write(&file, "scratch data")?;

    let read_back = fs::read_to_string(&file)?;
    println!("roundtrip ok: {}", read_back == "scratch data");
    println!("dir existed: {}", dir.path().is_dir());
    Ok(())
}
