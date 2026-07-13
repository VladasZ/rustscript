#!/usr/bin/env rust

use anyhow::Result;
use std::fs;

fn count_files(dir: &str) -> Result<i64> {
    let mut total = 0;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            total += count_files(&path.display().to_string())?;
        } else {
            total += 1;
        }
    }
    Ok(total)
}

fn main() -> Result<()> {
    let base = "/tmp/rustscript_walk_demo";
    fs::create_dir_all(base)?;
    fs::create_dir_all(format!("{base}/sub"))?;
    fs::write(format!("{base}/a.txt"), "1")?;
    fs::write(format!("{base}/b.rs"), "2")?;
    fs::write(format!("{base}/sub/c.rs"), "3")?;

    let mut names = Vec::new();
    let mut dirs = 0;
    for entry in fs::read_dir(base)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            dirs += 1;
        } else {
            names.push(entry.file_name().to_string_lossy().to_string());
        }
    }
    names.sort();

    println!("subdirectories: {dirs}");
    println!("files here: {:?}", names);
    println!("total files including sub: {}", count_files(base)?);
    Ok(())
}
