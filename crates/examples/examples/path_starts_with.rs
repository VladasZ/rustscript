#!/usr/bin/env rust

// `Path::starts_with` and `Path::ends_with` compare whole components, not
// characters. That is the difference that matters: "/a/bc" does not start with
// "/a/b", though the `str` method of the same name would say it does. So this is
// the right way to ask whether one path is inside another, and a string prefix
// test is not.
//
// The shape this came from: deciding whether a build directory was configured
// from the checkout being built, where a sibling named mod18023 sits next to one
// named mod180 and a character prefix would confuse the two.

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

    // The component rule, where a character prefix test would be wrong.
    let sibling = Path::new("/home/user/draco/mod18023");
    println!(
        "component not char prefix: {}",
        sibling.starts_with("/home/user/draco/mod180")
    );
    println!(
        "str would say: {}",
        "/home/user/draco/mod18023".starts_with("/home/user/draco/mod180")
    );

    // A path always starts with itself and with the root, and never with a
    // longer path.
    println!("self: {}", sibling.starts_with("/home/user/draco/mod18023"));
    println!("root: {}", sibling.starts_with("/"));
    println!(
        "longer: {}",
        sibling.starts_with("/home/user/draco/mod18023/S30")
    );

    // ends_with matches trailing components, so it takes a suffix of the path
    // rather than a file extension.
    println!("ends dir: {}", cmake_home.ends_with("S30/cmake"));
    println!("ends leaf: {}", cmake_home.ends_with("cmake"));
    println!("ends partial: {}", cmake_home.ends_with("make"));

    // A relative path, and an empty pattern, which every path starts with.
    let rel = Path::new("S30/cmake");
    println!("relative: {}", rel.starts_with("S30"));
    println!("empty: {}", rel.starts_with(""));
}
