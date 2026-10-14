#!/usr/bin/env rust

use std::fs::{File, read_to_string, remove_file};
use std::io::{Read, Write};
use std::process::{Command, Stdio};

fn main() -> anyhow::Result<()> {
    let path = std::env::temp_dir().join("rustscript_file_try_clone.txt");

    let mut first = File::create(&path)?;
    let mut second = first.try_clone()?;
    first.write_all(b"one\n")?;
    second.write_all(b"two\n")?;
    print!("shared writes:\n{}", read_to_string(&path)?);

    let mut file = File::create(&path)?;
    file.write_all(b"0123456789")?;
    let mut reader = File::open(&path)?;
    let mut head = [0u8; 4];
    reader.read_exact(&mut head)?;
    let mut clone = reader.try_clone()?;
    let mut rest = String::new();
    clone.read_to_string(&mut rest)?;
    println!(
        "read head {} clone rest {rest}",
        String::from_utf8_lossy(&head)
    );

    let log = File::create(&path)?;
    let status = Command::new("sh")
        .args(["-c", "echo out; echo err >&2"])
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .status()?;
    print!("child {}:\n{}", status.success(), read_to_string(&path)?);

    remove_file(&path)?;
    Ok(())
}
