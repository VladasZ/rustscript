#!/usr/bin/env rust

// The shape comes from reading 1 setting out of a CMakeCache.txt.

fn main() {
    let cache = [
        "CMAKE_BUILD_TYPE:STRING=RelWithDebInfo",
        "CMAKE_HOME_DIRECTORY:INTERNAL=/home/user/draco/modules/S30/cmake",
        "CMAKE_INSTALL_PREFIX:PATH=/home/user/draco/bld/install",
    ];
    let home = cache
        .iter()
        .find_map(|l| l.strip_prefix("CMAKE_HOME_DIRECTORY:INTERNAL="));
    println!("home: {home:?}");

    let missing = cache.iter().find_map(|l| l.strip_prefix("NO_SUCH_KEY="));
    println!("missing: {missing:?}");

    // stops at the first Some
    let words = ["one", "22", "three", "44"];
    let first_number = words.iter().find_map(|w| w.parse::<i64>().ok());
    println!("first number: {first_number:?}");

    // nothing converts, and an empty run
    let none_convert = ["a", "b"];
    println!(
        "none convert: {:?}",
        none_convert.iter().find_map(|w| w.parse::<i64>().ok())
    );
    let empty: Vec<String> = Vec::new();
    println!(
        "empty: {:?}",
        empty.iter().find_map(|w| w.parse::<i64>().ok())
    );

    let numbers = [1, 3, 6, 7, 8];
    let doubled_even = numbers
        .iter()
        .find_map(|n| if n % 2 == 0 { Some(n * 10) } else { None });
    println!("first even times ten: {doubled_even:?}");

    // a lazy chain takes the other path through the interpreter
    let lazy = numbers
        .iter()
        .map(|n| n + 1)
        .find_map(|n| if n > 5 { Some(n * 2) } else { None });
    println!("lazy: {lazy:?}");
}
