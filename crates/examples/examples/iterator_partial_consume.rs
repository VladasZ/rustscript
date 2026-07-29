#!/usr/bin/env rust

// An iterator that is read part way and then handed on. What it already gave
// out must not come back a second time. Getting this wrong is quiet, the first
// item simply appears twice, once from `next` and once from whatever drains
// the rest.

fn main() {
    let mut words = "max 20x extra".split(' ');
    let first = words.next();
    let rest: Vec<&str> = words.collect();
    println!("split first: {first:?}");
    println!("split rest: {rest:?}");

    // The shape this came from, a leading word styled apart from the tail.
    let mut parts = "default_claude_max_20x"
        .trim_start_matches("default_claude_")
        .split('_');
    let head = parts.next().unwrap_or_default();
    let tail: Vec<&str> = parts.collect();
    println!("plan: {} ({})", head.to_uppercase(), tail.join(" "));

    let mut lines = "one\ntwo\nthree".lines();
    println!("first line: {:?}", lines.next());
    println!("second line: {:?}", lines.next());
    println!("remaining lines: {}", lines.count());

    let mut numbers = [1, 2, 3, 4].into_iter();
    println!("first number: {:?}", numbers.next());
    println!("rest sum: {}", numbers.sum::<i32>());

    let mut letters = "abc".chars();
    println!("first letter: {:?}", letters.next());
    println!("rest of letters: {}", letters.as_str());

    // `by_ref` lends the iterator out and hands it back consumed, so the two
    // halves below must not overlap.
    let mut rest = "a b c d e".split(' ');
    let head: Vec<&str> = rest.by_ref().take(2).collect();
    let tail: Vec<&str> = rest.collect();
    println!("by_ref head: {head:?}");
    println!("by_ref tail: {tail:?}");

    let mut empty = "".split(' ');
    println!("empty first: {:?}", empty.next());
    println!("empty again: {:?}", empty.next());
}
