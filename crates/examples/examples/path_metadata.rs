#!/usr/bin/env rust

//! `Path::metadata` and `PathBuf::metadata` give what `std::fs::metadata` gives. They follow a
//! link, and `symlink_metadata` reads the link itself.

use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::symlink_file as symlink;
use std::path::Path;

fn main() -> io::Result<()> {
    let dir = std::env::temp_dir().join("rustscript_path_metadata");
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    fs::create_dir_all(&dir)?;
    let file = dir.join("note.txt");
    fs::write(&file, "twelve bytes")?;

    let from_buf = file.metadata()?;
    println!("len: {}", from_buf.len());
    println!("file: {} dir: {}", from_buf.is_file(), from_buf.is_dir());
    let from_path = Path::new(&file).metadata()?;
    println!(
        "same len: {}",
        from_path.len() == fs::metadata(&file)?.len()
    );
    println!("dir: {}", dir.metadata()?.is_dir());
    println!(
        "dir by link view: {}",
        dir.as_path().symlink_metadata()?.is_dir()
    );

    let link = dir.join("link.txt");
    symlink(&file, &link)?;
    let followed = link.metadata()?;
    let itself = link.symlink_metadata()?;
    println!("followed: {} {}", followed.is_symlink(), followed.len());
    println!("itself: {}", itself.is_symlink());

    let missing = dir.join("missing.txt");
    println!("missing: {}", missing.metadata().is_err());
    println!("missing link: {}", missing.symlink_metadata().is_err());
    if let Err(error) = missing.as_path().metadata() {
        println!("kind: {:?}", error.kind());
    }

    fs::remove_dir_all(&dir)?;
    Ok(())
}
