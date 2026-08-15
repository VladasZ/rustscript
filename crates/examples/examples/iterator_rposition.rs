#!/usr/bin/env rust

// `rposition` finds the last match rather than the first, and still reports the
// index counted from the front. std reaches it by walking backwards, which needs
// a reversible iterator, so the interesting cases are the ones where the answer
// differs from `position` and where nothing matches at all.
//
// The shape this came from: a board console log where the closing verdict is
// printed once per run, and reading a log with several runs in it must land on
// the last one rather than the first.

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

    // A single element and an empty run, where an off by one would show.
    let one = [7];
    println!("single hit: {:?}", one.iter().rposition(|n| *n == 7));
    println!("single miss: {:?}", one.iter().rposition(|n| *n == 8));
    let empty: Vec<i32> = Vec::new();
    println!("empty: {:?}", empty.iter().rposition(|n| *n == 0));

    // The tail after the last match, which is what the caller usually wants next.
    if let Some(index) = last {
        let tail: Vec<&str> = lines[index..].to_vec();
        println!("tail: {tail:?}");
    }
}
