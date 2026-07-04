#!/usr/bin/env rust

use anyhow::Result;
use std::process::Command;

fn main() -> Result<()> {
    let out = Command::new("echo").arg("hello from a subprocess").output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    println!("stdout: {}", text.trim());
    println!("success: {}", out.status.success());

    let uname = Command::new("uname").output()?;
    println!("kernel: {}", String::from_utf8_lossy(&uname.stdout).trim());
    Ok(())
}
