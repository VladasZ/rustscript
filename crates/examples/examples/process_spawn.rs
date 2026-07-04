#!/usr/bin/env rust

// Spawn a child, stream its stdout line by line while it runs, then feed a
// second child through its stdin and collect the output.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn main() -> anyhow::Result<()> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg("echo one; echo two; echo three")
        .stdout(Stdio::piped())
        .spawn()?;
    let out = child.stdout.take().unwrap();
    for line in BufReader::new(out).lines() {
        println!("line: {}", line?);
    }
    child.wait()?;

    let mut cat = Command::new("cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    cat.stdin.take().unwrap().write_all(b"fed through stdin\n")?;
    let output = cat.wait_with_output()?;
    print!("cat saw: {}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}
