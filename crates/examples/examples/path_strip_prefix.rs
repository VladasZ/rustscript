#!/usr/bin/env rust

//! `strip_prefix` compares whole components and takes a `&str`, a `PathBuf` or a `Path`.

use std::path::{Path, PathBuf};

fn main() {
    let full = Path::new("/a/b/c.md");
    println!("{:?}", full.strip_prefix(Path::new("/a")));
    println!("{:?}", full.strip_prefix("/a/b/"));
    println!("{:?}", full.strip_prefix("/x"));
    let base = PathBuf::from("/a");
    match full.strip_prefix(&base) {
        Ok(rest) => println!("{} {:?}", rest.display(), rest.parent()),
        Err(e) => println!("{e}"),
    }
    if let Err(e) = full.strip_prefix("/a/bc") {
        println!("{e} {e:?}");
    }
    let owned = PathBuf::from("/a/b/c.md");
    let rest = owned
        .strip_prefix("/a")
        .map(Path::to_path_buf)
        .unwrap_or_default();
    println!("{}", rest.display());
    println!("{:?}", Path::new("a/b").strip_prefix(""));
    println!("{:?}", Path::new("/a/b").strip_prefix("/a/b"));
}
