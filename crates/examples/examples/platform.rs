#!/usr/bin/env rust

// Detect the operating system and find standard directories.

use std::env::consts;

use dirs::{cache_dir, home_dir};

fn main() {
    let os = consts::OS;
    println!("known os: {}", matches!(os, "macos" | "linux" | "windows"));
    println!("arch nonempty: {}", !consts::ARCH.is_empty());

    let home = home_dir();
    println!("home found: {}", home.is_some());

    let cache = cache_dir();
    println!("cache found: {}", cache.is_some());
}
