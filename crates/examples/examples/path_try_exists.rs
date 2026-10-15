#!/usr/bin/env rust

//! `Path::try_exists` and `fs::exists` report an I/O error instead of hiding it as `false`, and
//! `MetadataExt::nlink` counts the hard links of a file.

use std::fs;

fn main() -> std::io::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("note.txt");
    println!("before: {} {}", file.try_exists()?, fs::exists(&file)?);
    fs::write(&file, "hi")?;
    println!("after: {} {}", file.try_exists()?, fs::exists(&file)?);
    println!("dir: {}", dir.path().try_exists()?);

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        println!("links: {}", fs::metadata(&file)?.nlink());
        fs::hard_link(&file, dir.path().join("twin.txt"))?;
        let meta = fs::symlink_metadata(&file)?;
        println!("links: {} linked: {}", meta.nlink(), meta.nlink() > 1);
        println!(
            "is_ok_and: {}",
            fs::symlink_metadata(&file).is_ok_and(|m| m.nlink() > 1)
        );
    }
    Ok(())
}
