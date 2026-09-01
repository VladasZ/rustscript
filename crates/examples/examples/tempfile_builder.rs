#!/usr/bin/env rust

//! `tempfile::Builder` with a prefix and suffix, the shape the github-cli scripts use to stage
//! JSON payloads for `gh api --input`.

use std::io::Write;

fn main() {
    let dir = tempfile::tempdir().unwrap();
    let mut file = tempfile::Builder::new()
        .prefix("payload_")
        .suffix(".json")
        .tempfile_in(dir.path())
        .unwrap();
    file.write_all(b"{\"ok\":true}").unwrap();
    file.flush().unwrap();

    let name = file
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    println!(
        "{} {}",
        name.starts_with("payload_"),
        std::path::Path::new(&name)
            .extension()
            .is_some_and(|e| e == "json")
    );
    println!("{}", std::fs::read_to_string(file.path()).unwrap());
    println!(
        "{}",
        tempfile::Builder::new()
            .tempfile_in("/definitely/missing/dir")
            .is_err()
    );
}
