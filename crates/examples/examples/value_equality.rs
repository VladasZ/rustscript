//! Equality on values that share storage. A clone shares its mutex, so a comparison must not relock a
//! held mutex. The struct arm matters for `PathBuf` comparisons like `cwd == home()`.

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

    // the shape that froze `cm`, `cb`, `cl` and `ll-isolate`
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

    // NaN is not equal to its own clone
    let nan = vec![f64::NAN];
    println!("nan: {}", nan == nan.clone());

    let pair = (1, String::from("x"));
    println!("tuple: {}", pair == pair.clone());

    let nested = vec![vec![1, 2], vec![3]];
    println!("nested: {}", nested == nested.clone());

    // `dedup` and `contains` compare elements that share storage
    let mut paths = vec![path.clone(), path.clone(), PathBuf::from("/z")];
    paths.dedup();
    println!("dedup: {} {}", paths.len(), paths.contains(&path));
}
