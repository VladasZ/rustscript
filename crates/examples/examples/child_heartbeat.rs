#!/usr/bin/env rust

// Polling a child with `try_wait`. The output goes to a file through
// `Stdio::from`, because a pipe nobody drains deadlocks. Nothing is printed
// inside the poll loop because the poll count depends on machine speed.

use std::fs::{File, read_to_string};
use std::io::Write;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

fn poll_to_end(mut child: std::process::Child) -> (Option<i32>, bool) {
    let mut polled = false;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return (status.code(), polled),
            Ok(None) => {
                polled = true;
                sleep(Duration::from_millis(100));
            }
            Err(e) => {
                println!("poll failed: {e}");
                return (None, polled);
            }
        }
    }
}

fn main() {
    let path = std::env::temp_dir().join("rustscript-child-heartbeat.txt");

    let out = File::create(&path).expect("create the capture file");
    let child = Command::new("sh")
        .arg("-c")
        .arg("sleep 1; echo first; echo second")
        .stdout(Stdio::from(out))
        .spawn()
        .expect("spawn");

    let (code, polled) = poll_to_end(child);
    println!("exit {code:?}, polled while it ran {polled}");

    let captured = read_to_string(&path).expect("read the capture file");
    let mut lines: Vec<&str> = Vec::new();
    for line in captured.lines() {
        lines.push(line);
    }
    println!("captured {} lines: {}", lines.len(), lines.join(","));

    // The script's own handle survives the child taking a clone of it.
    let mut again = File::create(&path).expect("reopen");
    let how = "writeln";
    writeln!(again, "rewritten by {how}").expect("write");
    write!(again, "and by write, no newline").expect("write");
    println!("after reuse: {:?}", read_to_string(&path).expect("reread"));

    std::fs::remove_file(&path).ok();
}
