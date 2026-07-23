#!/usr/bin/env rust

// Open a file with the low level API, write, seek back to the start, and read
// it again through a buffered reader.

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

    // OpenOptions append does not truncate, so the earlier content stays.
    let mut appended = OpenOptions::new().create(true).append(true).open(&path)?;
    appended.write_all(b"third line\n")?;
    let after = fs::read_to_string(&path)?;
    println!("lines after append: {}", after.lines().count());

    // Copy the file and stamp the source mtime onto it. The call is what exercises
    // set_modified; SystemTime values are not printed because the two engines model
    // their identity differently, which would break the byte-for-byte comparison.
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
