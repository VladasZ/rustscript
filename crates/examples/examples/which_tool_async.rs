#!/usr/bin/env rust

// Look up an executable on PATH from a tokio script, so the crate bridge
// is proven under the async surface too and can still
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
