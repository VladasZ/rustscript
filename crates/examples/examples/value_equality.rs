//! Equality and ordering on values that share storage inside the interpreter.
//! A script clone of a container or struct shares its mutex, so every
//! comparison here once relocked a mutex it already held and deadlocked.
//! Compiled Rust deep copies instead, so the outputs must still match byte
//! for byte. The struct arm also held both sides locked while reading fields,
//! which hung every comparison of two `PathBuf`s, the exact shape of the
//! `cwd == home()` check that froze real scripts.

use std::env;
use std::path::PathBuf;

#[derive(Clone, PartialEq)]
struct Point {
    x: i64,
    y: i64,
}

fn main() {
    let first = Point { x: 1, y: 2 };
    let copy = first.clone();
    let other = Point { x: 3, y: 2 };
    println!(
        "struct: {} {} {} {}",
        first == copy,
        first == other,
        first == first.clone(),
        copy == other
    );

    let path = PathBuf::from("/a/b");
    let same = path.clone();
    let different = PathBuf::from("/a/c");
    println!(
        "path: {} {} {}",
        path == same,
        path == different,
        path == path.clone()
    );

    // The shape that froze cm, cb, cl, and ll-isolate: a Result adapter whose
    // closure compares two paths.
    let nowhere = PathBuf::from("/nowhere");
    let at_root = env::current_dir().is_ok_and(|cwd| cwd == nowhere);
    println!("cwd: {at_root}");

    let numbers = vec![1, 2, 3];
    println!(
        "vec: {} {} {}",
        numbers == numbers.clone(),
        numbers <= numbers.clone(),
        numbers < numbers.clone()
    );

    // NaN makes equality false even against the value's own clone.
    let nan = vec![f64::NAN];
    println!("nan: {}", nan == nan.clone());

    let pair = (1, String::from("x"));
    println!("tuple: {}", pair == pair.clone());

    let nested = vec![vec![1, 2], vec![3]];
    println!("nested: {}", nested == nested.clone());

    // dedup and contains compare elements that share storage with each other
    // and with the needle.
    let mut paths = vec![path.clone(), path.clone(), PathBuf::from("/z")];
    paths.dedup();
    println!("dedup: {} {}", paths.len(), paths.contains(&path));
}
