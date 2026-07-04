#!/usr/bin/env rust

// Detect the operating system and find standard directories.

use std::env::consts;

fn main() {
    let os = consts::OS;
    println!("known os: {}", matches!(os, "macos" | "linux" | "windows"));
    println!("arch nonempty: {}", !consts::ARCH.is_empty());

    let home = dirs::home_dir();
    println!("home found: {}", home.is_some());

    let cache = dirs::cache_dir();
    println!("cache found: {}", cache.is_some());
}
