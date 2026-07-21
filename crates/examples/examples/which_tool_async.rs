#!/usr/bin/env rust

// Look up an executable on PATH from a tokio script. The parallel engine has
// its own bridge table, so a crate call that works in the fast engine can still
// be missing here. This is the twin of which_tool.rs that covers that engine.

#[tokio::main]
async fn main() {
    match which::which("cargo") {
        Ok(_) => println!("cargo on path: true"),
        Err(_) => println!("cargo on path: false"),
    }
    println!(
        "missing tool found: {}",
        which::which("definitely-not-a-real-tool").is_ok()
    );
}
