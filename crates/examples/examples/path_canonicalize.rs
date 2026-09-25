#!/usr/bin/env rust

// `Path::canonicalize` resolves `.` and `..` against the real file system and fails on a missing
// path, like `fs::canonicalize`.

use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let here = Path::new(".").canonicalize().unwrap();
    let cwd = env::current_dir().unwrap();
    println!("absolute: {}", here.is_absolute());
    println!(
        "same as fs: {}",
        here.display().to_string() == fs::canonicalize(".").unwrap().display().to_string()
    );
    println!(
        "same as cwd: {}",
        here.display().to_string() == cwd.canonicalize().unwrap().display().to_string()
    );

    let up = cwd.join("..").canonicalize().unwrap();
    let parent = here.parent().unwrap();
    println!(
        "dot dot is parent: {}",
        up.display().to_string() == parent.display().to_string()
    );

    let missing = Path::new("./no-such-dir-for-canonicalize");
    match missing.canonicalize() {
        Ok(p) => println!("unexpected {}", p.display()),
        Err(e) => println!("missing: {:?}", e.kind()),
    }
}
