#!/usr/bin/env rust

// Cutting a list and a string in 2, and the std rules for equal items in a search and a dedup.

fn main() {
    let ports: Vec<u16> = "22 80 443 8080 9000"
        .split(' ')
        .filter_map(|port| port.parse().ok())
        .collect();
    let (low, high) = ports.split_at(3);
    println!("low {low:?}, high {high:?}");
    let (none, all) = ports.split_at(0);
    println!("none {none:?}, all {}", all.len());
    let (whole, rest) = ports.split_at(ports.len());
    println!("whole {}, rest {rest:?}", whole.len());

    match ports.split_last() {
        Some((last, before)) => println!("last {last}, before {before:?}"),
        None => println!("no ports"),
    }
    let empty: Vec<u16> = Vec::new();
    println!("empty split_last: {:?}", empty.split_last());
    println!("single split_last: {:?}", [7].split_last());

    let line = "key=value";
    let (key, tail) = line.split_at(3);
    println!("key {key}, tail {tail}");
    println!("{:?}", "héllo".split_at(3));
    println!("{:?}", "abc".split_at(0));
    println!("{:?}", "abc".split_at(3));

    // among equal items std lands on one fixed index
    for len in 0..9 {
        let same = vec![5u8; len];
        println!(
            "{len} equal: {:?} {:?} {:?}",
            same.binary_search(&5),
            same.binary_search(&4),
            same.binary_search(&6)
        );
    }
    let steps = [1, 2, 2, 2, 3, 5, 5, 8, 8, 8, 8, 13];
    for needle in 0..15 {
        print!("{:?} ", steps.binary_search(&needle));
    }
    println!();

    // the key closure runs for both sides of every comparison, the later item first
    let mut calls = 0;
    let mut seen = Vec::new();
    let mut levels = vec![10, 11, 20, 21, 22, 30];
    levels.dedup_by_key(|level| {
        calls += 1;
        seen.push(*level);
        *level / 10
    });
    println!("levels {levels:?}, calls {calls}, seen {seen:?}");

    let mut turn = 0usize;
    let mut flat = vec![4i16; 5];
    flat.dedup_by_key(|_| {
        turn += 1;
        turn / 2
    });
    println!("flat {flat:?}, turn {turn}");
}
