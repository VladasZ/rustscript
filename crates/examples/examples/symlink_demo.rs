#!/usr/bin/env rust

// Create a symlink and read back where it points.

use std::fs;

fn main() -> anyhow::Result<()> {
    let dir = std::env::temp_dir();
    let target = dir.join("rustscript_link_target.txt");
    let link = dir.join("rustscript_link.txt");
    let target = target.to_string_lossy().to_string();
    let link = link.to_string_lossy().to_string();

    fs::write(&target, "point here")?;
    if std::path::Path::new(&link).exists() {
        fs::remove_file(&link)?;
    }
    std::os::unix::fs::symlink(&target, &link)?;

    let pointed = fs::read_link(&link)?;
    println!("link resolves to target: {}", pointed.to_string_lossy() == target);

    let meta = fs::symlink_metadata(&link)?;
    println!("is_symlink: {}", meta.is_symlink());

    fs::remove_file(&link)?;
    fs::remove_file(&target)?;
    Ok(())
}
