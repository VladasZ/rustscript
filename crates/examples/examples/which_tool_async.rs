#!/usr/bin/env rust

// The `#[tokio::main]` twin of `which_tool`.

use which::which;

#[tokio::main]
async fn main() {
    match which("cargo") {
        Ok(_) => println!("cargo on path: true"),
        Err(_) => println!("cargo on path: false"),
    }
    println!(
        "missing tool found: {}",
        which("definitely-not-a-real-tool").is_ok()
    );
}
