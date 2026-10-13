#!/usr/bin/env rust

// A by value binding behind a `&` pattern is a copy of a `Copy` value. The original stays
// whole when the copy is written, dropped or handed to a call. A `Copy` value in a capture
// cell reads as a copy too, and a borrowed argument a method keeps stays with the caller.

use std::collections::HashMap;

struct Trace(u8);

impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

#[derive(Clone, Copy, Debug)]
struct Point {
    x: i64,
    tag: Option<u8>,
}

/// Builds and drops a vec of the value, the drop reaches whatever shares its storage.
fn repeated<T: Clone>(value: T, count: usize) -> usize {
    vec![value; count].len()
}

fn widened(&pair: &(i64, Option<u8>)) -> i64 {
    let mut own = pair;
    own.0 += 100;
    own.1 = None;
    own.0 + i64::from(pair.1.unwrap_or(0))
}

fn count_twice(key: &i128) -> usize {
    let mut seen: HashMap<&i128, &i128> = HashMap::new();
    seen.insert(key, key);
    seen.insert(key, key);
    seen.len() + usize::try_from(*key).unwrap_or(0)
}

fn main() {
    let trace = Trace(1);
    println!("start {}", trace.0);

    let options = vec![Some(1i64), None, Some(3)];
    let kept: Vec<Option<i64>> = options
        .clone()
        .into_iter()
        .filter(|&item: &Option<i64>| repeated(item, 2) == 2)
        .collect();
    println!("{kept:?}");

    let pairs = vec![(Some(1i16), 2usize), (None, 5)];
    let sizes: Vec<usize> = pairs
        .iter()
        .map(|&(key, value): &(Option<i16>, usize)| value + repeated(key, 3))
        .collect();
    println!("{sizes:?} {pairs:?}");

    let points = vec![Point { x: 1, tag: Some(2) }];
    let moved: Vec<i64> = points
        .iter()
        .map(|&point| {
            let mut own = point;
            own.x += 5;
            own.tag = None;
            own.x + i64::from(own.tag.unwrap_or(0))
        })
        .collect();
    println!("{moved:?} {points:?}");

    let mut lens = Vec::new();
    for &item in &options {
        lens.push(repeated(item, 1));
    }
    for (index, &item) in options.iter().enumerate() {
        lens.push(index + repeated(item, 1));
    }
    let whole = Some((1u8, 'c'));
    let &copy = &whole;
    lens.push(repeated(copy, 2));
    if let Some(&item) = options.first() {
        lens.push(repeated(item, 4));
    }
    match options.get(2) {
        Some(&item) => lens.push(repeated(item, 5)),
        None => lens.push(0),
    }
    println!("{lens:?} {options:?} {whole:?} {copy:?}");

    let pair = (7i64, Some(1u8));
    println!("{} {pair:?}", widened(&pair));

    let ids = [4i64, 5];
    let once: Vec<bool> = ids
        .iter()
        .map(|&id: &i64| {
            let mut seen: HashMap<i64, i64> = HashMap::new();
            seen.insert(id, id);
            seen.insert(id, 1);
            seen.len() == 1
        })
        .collect();
    println!("{once:?} {}", count_twice(&4));

    let text = String::from("abc");
    let mut held: Vec<&String> = Vec::new();
    println!("{}", held.len());
    held.push(&text);
    println!("{text} {held:?}");

    let mut counted = (1u8, Some(2u8));
    let mut bump = || {
        counted.0 += 1;
        counted.0
    };
    println!("{}", bump());
    println!("{} {counted:?}", repeated(counted, 2));
}
