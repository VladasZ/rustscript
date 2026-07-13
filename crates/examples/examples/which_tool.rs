#!/usr/bin/env rust

// Look up an executable on PATH.

fn main() {
    match which::which("cargo") {
        Ok(_) => println!("cargo on path: true"),
        Err(_) => println!("cargo on path: false"),
    }
    println!(
        "missing tool found: {}",
        which::which("definitely-not-a-real-tool").is_ok()
    );
}
