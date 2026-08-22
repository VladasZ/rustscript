#!/usr/bin/env rust

// `VecDeque`, `binary_search`, `abs_diff` and `write!` into a `String`.

use std::collections::VecDeque;
use std::fmt::Write as _;

fn main() {
    let mut queue: VecDeque<i32> = VecDeque::new();
    queue.push_back(1);
    queue.push_front(0);
    queue.push_back(2);
    println!(
        "{:?} {} {:?} {:?}",
        queue,
        queue.len(),
        queue.front(),
        queue.back()
    );
    if let Some(first) = queue.front_mut() {
        *first = 10;
    }
    while let Some(x) = queue.pop_front() {
        print!("{x} ");
    }
    println!("{:?} {}", queue.pop_back(), queue.is_empty());

    let mut sorted = VecDeque::from(vec![3, 1, 2]);
    sorted.make_contiguous().sort_unstable();
    let items: Vec<i32> = sorted.iter().copied().collect();
    println!("{items:?} {}", sorted.contains(&2));

    let data = [1, 2, 3, 5];
    println!(
        "{:?} {:?} {:?}",
        data.binary_search(&3),
        data.binary_search(&4),
        data.binary_search(&0)
    );
    match data.binary_search(&9) {
        Ok(at) => println!("found at {at}"),
        Err(at) => println!("insert at {at}"),
    }

    println!(
        "{} {} {} {}",
        7u32.abs_diff(10),
        (-5i32).abs_diff(5),
        3i8.abs_diff(-100),
        200u8.abs_diff(1)
    );
    let gap: u32 = 3i32.abs_diff(9);
    println!("{gap}");

    let mut out = String::new();
    let tag = "a";
    write!(out, "{:>5}|{tag}", 42).unwrap();
    writeln!(out, " end").unwrap();
    write!(&mut out, "more").unwrap();
    println!("{out}");
    let mut line = String::from("x");
    for i in 0..3 {
        write!(line, "{i}").unwrap();
    }
    println!("{line}");
}
