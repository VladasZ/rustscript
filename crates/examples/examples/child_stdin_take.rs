#!/usr/bin/env rust

// Guards the hidden stdin alias in `spawn_command`. Without it this
// deadlocks in `wait_with_output` with `cat` waiting for input forever.

use std::io::Write;
use std::process::{Command, Stdio};

fn main() {
    let mut child = Command::new("cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn cat");

    let mut stdin = child.stdin.take().expect("stdin is piped");
    println!("field after take: {}", child.stdin.is_none());
    stdin.write_all(b"first\n").expect("write first");
    stdin.write_all(b"second\n").expect("write second");
    drop(stdin);

    let out = child.wait_with_output().expect("wait");
    print!("{}", String::from_utf8_lossy(&out.stdout));
    println!("exit ok: {}", out.status.success());
}
