#!/usr/bin/env rust

// The shape comes from a board console log with several runs in it, the last verdict is the one
// that counts.

fn main() {
    let lines = ["start", "OK (2)", "middle", "OK (165)", "done"];

    let first = lines.iter().position(|l| l.starts_with("OK ("));
    let last = lines.iter().rposition(|l| l.starts_with("OK ("));
    println!("position: {first:?}");
    println!("rposition: {last:?}");

    let missing = lines.iter().rposition(|l| l.starts_with("Run:"));
    println!("no match: {missing:?}");

    let numbers = [1, 4, 2, 4, 3];
    println!("last four: {:?}", numbers.iter().rposition(|n| *n == 4));
    println!("last odd: {:?}", numbers.iter().rposition(|n| n % 2 == 1));

    // a single element and an empty run
    let one = [7];
    println!("single hit: {:?}", one.iter().rposition(|n| *n == 7));
    println!("single miss: {:?}", one.iter().rposition(|n| *n == 8));
    let empty: Vec<i32> = Vec::new();
    println!("empty: {:?}", empty.iter().rposition(|n| *n == 0));

    // the tail after the last match
    if let Some(index) = last {
        let tail: Vec<&str> = lines[index..].to_vec();
        println!("tail: {tail:?}");
    }
}
