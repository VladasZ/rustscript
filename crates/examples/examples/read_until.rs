#!/usr/bin/env rust

// `lines()` yields an Err on a bad byte and the caller cannot tell that from
// end of output, so one cp1252 byte truncates the capture. `read_until` plus
// `from_utf8_lossy` survives it.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

fn capture(mut reader: impl BufRead) -> String {
    let mut captured = String::new();
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        while matches!(buf.last(), Some(b'\n' | b'\r')) {
            buf.pop();
        }
        let line = String::from_utf8_lossy(&buf);
        captured.push_str(&line);
        captured.push('\n');
    }
    captured
}

fn main() -> anyhow::Result<()> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(r"printf 'first\nlatin1 \344 byte\nlast\n'")
        .stdout(Stdio::piped())
        .spawn()?;
    let out = child.stdout.take().unwrap();
    let captured = capture(BufReader::new(out));
    child.wait()?;

    for line in captured.lines() {
        println!("line: {line}");
    }
    println!("lines: {}", captured.lines().count());
    println!("survived the bad byte: {}", captured.contains("last"));
    Ok(())
}
