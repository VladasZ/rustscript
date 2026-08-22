#!/usr/bin/env rust

// `Path::starts_with` compares whole components, so "/a/bc" doesn't start with "/a/b". The shape
// comes from telling a `mod18023` checkout from a `mod180` one.

use std::path::Path;

fn main() {
    let cmake_home = Path::new("/home/user/draco/mod18023/S30/cmake");
    println!(
        "inside sibling: {}",
        cmake_home.starts_with("/home/user/draco/mod18023")
    );
    println!(
        "inside canonical: {}",
        cmake_home.starts_with("/home/user/draco/modules")
    );

    // the component rule
    let sibling = Path::new("/home/user/draco/mod18023");
    println!(
        "component not char prefix: {}",
        sibling.starts_with("/home/user/draco/mod180")
    );
    println!(
        "str would say: {}",
        "/home/user/draco/mod18023".starts_with("/home/user/draco/mod180")
    );

    // itself, the root, and a longer path
    println!("self: {}", sibling.starts_with("/home/user/draco/mod18023"));
    println!("root: {}", sibling.starts_with("/"));
    println!(
        "longer: {}",
        sibling.starts_with("/home/user/draco/mod18023/S30")
    );

    // `ends_with` takes trailing components, not an extension
    println!("ends dir: {}", cmake_home.ends_with("S30/cmake"));
    println!("ends leaf: {}", cmake_home.ends_with("cmake"));
    println!("ends partial: {}", cmake_home.ends_with("make"));

    // a relative path and an empty pattern
    let rel = Path::new("S30/cmake");
    println!("relative: {}", rel.starts_with("S30"));
    println!("empty: {}", rel.starts_with(""));
}
