#!/usr/bin/env rust

// Read a file's metadata: size and kind. The size is fixed by what we write, so
// it is stable across runs.

use std::fs;

fn main() -> anyhow::Result<()> {
    let path = std::env::temp_dir().join("rustscript_meta.txt");
    let path = path.to_string_lossy().to_string();
    fs::write(&path, "twelve bytes")?;

    let m = fs::metadata(&path)?;
    println!("len: {}", m.len());
    println!("is_file: {}", m.is_file());
    println!("is_dir: {}", m.is_dir());
    println!("is_symlink: {}", m.is_symlink());

    fs::remove_file(&path)?;
    Ok(())
}
