#!/usr/bin/env rust

use std::ffi::OsString;
use std::path::{Path, PathBuf};

fn main() {
    let empty = PathBuf::new();
    let full = PathBuf::from("round/icon.svg");
    println!(
        "{} {}",
        empty.as_os_str().is_empty(),
        full.as_os_str().is_empty()
    );
    println!("{} {}", empty.as_os_str().len(), full.as_os_str().len());

    let os = full.as_os_str();
    println!("{:?}", os.to_str());
    println!("{}", os.to_string_lossy());
    println!("{:?}", (&full, Path::new("a\"b"), Some(empty.clone()), os));
    let owned: OsString = os.to_os_string();
    println!("{:?} {}", (&owned,), owned.len());
    println!("{:?}", owned.clone().into_string());
    println!("{}", Path::new(&owned).display());

    let back: OsString = full.clone().into_os_string();
    println!("{:?} {}", back.to_str(), back.is_empty());
    println!("{}", PathBuf::from(back).join("x").display());

    if let Some(home) = std::env::var_os("RS_OS_STR_UNSET") {
        println!("set {}", home.len());
    } else {
        println!("unset");
    }
}
