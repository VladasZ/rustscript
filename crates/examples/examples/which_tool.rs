#!/usr/bin/env rust

// Look up an executable on PATH.

use which::which;

fn main() {
    match which("cargo") {
        Ok(_) => println!("cargo on path: true"),
        Err(_) => println!("cargo on path: false"),
    }
    println!(
        "missing tool found: {}",
        which("definitely-not-a-real-tool").is_ok()
    );
}
