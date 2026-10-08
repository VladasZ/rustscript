#!/usr/bin/env rust

// A range kept in a binding is an iterator with its own position.

use std::ops::Range;

struct Pages {
    left: Range<u32>,
}

fn take_two(ids: &mut Range<u32>) -> u32 {
    ids.next().unwrap_or(0) + ids.next().unwrap_or(0)
}

fn main() {
    let mut ids = 0..10;
    println!("{:?} {:?}", ids.next(), ids.next());
    println!("left {ids:?}, {} more", ids.len());

    let mut ids = 0..10;
    let first = ids.by_ref().next();
    println!("{first:?} then {:?}", ids.next());

    let mut ids = 0..10;
    let sum: i32 = ids.by_ref().take(3).sum();
    println!("sum {sum}, then {:?}, left {ids:?}", ids.next());

    let mut days = 1..=5;
    for day in days.by_ref() {
        if day == 2 {
            break;
        }
    }
    println!("{:?}", days.collect::<Vec<i32>>());

    let mut bytes = 1..=5u8;
    println!(
        "{:?} {:?} {:?}",
        bytes.next_back(),
        bytes.nth(1),
        bytes.next()
    );
    println!("{bytes:?}");

    let mut short = 0..2;
    println!("{:?} {:?} {:?}", short.next(), short.next(), short.next());
    println!("{short:?} empty {}", short.is_empty());

    let mut pages = Pages { left: 5..8 };
    println!(
        "{:?} {:?} {:?}",
        pages.left.next(),
        pages.left.next(),
        pages.left
    );

    let mut ids: Range<u32> = 1..9;
    println!("{} {ids:?}", take_two(&mut ids));

    let mut spans = vec![0..2, 5..7];
    println!("{:?} {spans:?}", spans[1].next());
}
