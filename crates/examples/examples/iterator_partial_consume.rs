#!/usr/bin/env rust

// An iterator read part way and handed on must not give the first item twice.

fn main() {
    let mut words = "max 20x extra".split(' ');
    let first = words.next();
    let rest: Vec<&str> = words.collect();
    println!("split first: {first:?}");
    println!("split rest: {rest:?}");

    let mut parts = "default_plan_max_20x"
        .trim_start_matches("default_plan_")
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

    // the 2 halves must not overlap
    let mut rest = "a b c d e".split(' ');
    let head: Vec<&str> = rest.by_ref().take(2).collect();
    let tail: Vec<&str> = rest.collect();
    println!("by_ref head: {head:?}");
    println!("by_ref tail: {tail:?}");

    // an adapter over `by_ref` that goes away leaves the rest in the iterator
    let mut numbers = vec![1, 2, 3, 4].into_iter();
    let head = numbers.by_ref().take(1).collect::<Vec<i32>>();
    println!(
        "vec head: {head:?}, rest: {:?}",
        numbers.collect::<Vec<i32>>()
    );

    let mut numbers = vec![1, 2, 3, 4].into_iter();
    let head: Vec<i32> = numbers.by_ref().take(2).collect();
    println!("vec head: {head:?}, next: {:?}", numbers.next());

    let mut numbers = vec![1, 2, 3, 4].into_iter();
    let small = numbers.by_ref().take_while(|x| *x < 2).count();
    println!("small: {small}, rest: {:?}", numbers.collect::<Vec<i32>>());

    let mut numbers = vec![1, 2, 3, 4].into_iter();
    let second: Vec<i32> = numbers.by_ref().skip(1).take(1).collect();
    println!(
        "second: {second:?}, rest: {:?}",
        numbers.collect::<Vec<i32>>()
    );

    let mut numbers = vec![1, 2, 3, 4].into_iter();
    let count = numbers.by_ref().take(1).count();
    println!("count: {count}, next: {:?}", numbers.next());

    let mut numbers = vec![1, 2, 3, 4].into_iter();
    let mapped = numbers.by_ref().map(|x| x + 1).next();
    println!("mapped: {mapped:?}, next: {:?}", numbers.next());

    let mut numbers = vec![1, 2, 3, 4].into_iter();
    for number in numbers.by_ref().take(2) {
        print!("{number} ");
    }
    println!("then {:?}", numbers.collect::<Vec<i32>>());

    let mut empty = "".split(' ');
    println!("empty first: {:?}", empty.next());
    println!("empty again: {:?}", empty.next());
}
