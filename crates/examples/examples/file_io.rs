#!/usr/bin/env rust


use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};

fn main() -> anyhow::Result<()> {
    let path = std::env::temp_dir().join("rustscript_file_io.txt");
    let path = path.to_string_lossy().to_string();

    let mut f = File::create(&path)?;
    f.write_all(b"first line\n")?;
    f.write_all(b"second line\n")?;
    f.flush()?;

    f.seek(SeekFrom::Start(0))?;
    let mut contents = String::new();
    let mut reader = File::open(&path)?;
    reader.read_to_string(&mut contents)?;

    println!("bytes written: {}", contents.len());
    println!("lines: {}", contents.lines().count());

    // append doesn't truncate
    let mut appended = OpenOptions::new().create(true).append(true).open(&path)?;
    appended.write_all(b"third line\n")?;
    let after = fs::read_to_string(&path)?;
    println!("lines after append: {}", after.lines().count());

    // `SystemTime` values are not printed, their identity differs between the interpreter and
    // compiled Rust
    let copy = std::env::temp_dir().join("rustscript_file_io_copy.txt");
    let copy = copy.to_string_lossy().to_string();
    fs::copy(&path, &copy)?;
    let mtime = fs::metadata(&path)?.modified()?;
    OpenOptions::new()
        .write(true)
        .open(&copy)?
        .set_modified(mtime)?;
    println!("copy bytes: {}", fs::metadata(&copy)?.len());

    std::fs::remove_file(&path)?;
    std::fs::remove_file(&copy)?;
    Ok(())
}
