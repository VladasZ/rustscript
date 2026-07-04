#!/usr/bin/env rust

// Match files with a glob pattern.

use std::fs;

fn main() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join("rustscript_glob");
    fs::create_dir_all(&dir)?;
    for name in ["a.txt", "b.txt", "c.log"] {
        fs::write(dir.join(name), "x")?;
    }

    let pattern = format!("{}/*.txt", dir.to_string_lossy());
    let mut count = 0;
    for entry in glob::glob(&pattern)? {
        entry?;
        count += 1;
    }
    println!("txt files matched: {}", count);

    fs::remove_dir_all(&dir)?;
    Ok(())
}
