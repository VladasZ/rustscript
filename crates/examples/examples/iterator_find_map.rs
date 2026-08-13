#!/usr/bin/env rust

// `find_map` is the short-circuiting pair of `filter_map`: it stops at the first
// element whose closure returns Some, and yields that inner value rather than
// the element. So it answers "the first thing that converts" in one pass,
// without the find-then-convert-again shape.
//
// The shape this came from: reading one setting out of a CMakeCache.txt, where
// the wanted line is found and stripped of its key in the same step.

fn main() {
    let cache = vec![
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

    // It stops at the first Some, so a later match never wins.
    let words = vec!["one", "22", "three", "44"];
    let first_number = words.iter().find_map(|w| w.parse::<i64>().ok());
    println!("first number: {first_number:?}");

    // Nothing converts, and an empty run, where an early return would show.
    let none_convert = vec!["a", "b"];
    println!(
        "none convert: {:?}",
        none_convert.iter().find_map(|w| w.parse::<i64>().ok())
    );
    let empty: Vec<String> = Vec::new();
    println!("empty: {:?}", empty.iter().find_map(|w| w.parse::<i64>().ok()));

    // The closure decides, so it can map to something unrelated to the element.
    let numbers = vec![1, 3, 6, 7, 8];
    let doubled_even = numbers.iter().find_map(|n| if n % 2 == 0 { Some(n * 10) } else { None });
    println!("first even times ten: {doubled_even:?}");

    // On a lazy chain rather than a materialised vec, which takes the other path
    // through the interpreter.
    let lazy = numbers.iter().map(|n| n + 1).find_map(|n| if n > 5 { Some(n * 2) } else { None });
    println!("lazy: {lazy:?}");
}
