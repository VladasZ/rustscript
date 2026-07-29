#!/usr/bin/env rust

// Polling a child while it runs, so a long silent command can print a progress heartbeat instead of
// looking hung. The shape comes from a real board tool whose first step was a `git status` over a
// Windows drive: 50k files, ten minutes, and not one byte of output to prove it was alive.
//
// Two pieces make it work. `try_wait` asks whether the child is done without blocking, so the caller
// keeps control between polls. And the output goes to a file through `Stdio::from`, not to a pipe,
// because a pipe nobody is draining deadlocks as soon as the child outgrows the buffer.
//
// Nothing is printed inside the poll loop on purpose. A real caller prints elapsed seconds there, but
// the number of polls depends on machine speed, and this example's output is compared byte for byte
// between a compiled and an interpreted run.

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

    // The script's own handle survives the child taking a clone of it, so the file is still writable.
    let mut again = File::create(&path).expect("reopen");
    let how = "writeln";
    writeln!(again, "rewritten by {how}").expect("write");
    write!(again, "and by write, no newline").expect("write");
    println!("after reuse: {:?}", read_to_string(&path).expect("reread"));

    std::fs::remove_file(&path).ok();
}
