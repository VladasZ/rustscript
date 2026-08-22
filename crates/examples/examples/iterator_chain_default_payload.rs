#!/usr/bin/env rust

//! A default at the end of an iterator chain finds its type through the chain.

#[derive(Debug, Clone, Default)]
struct Row {
    slot: Option<u16>,
    count: i64,
}

impl Row {
    fn describe(&self) -> String {
        format!("{:?}/{}", self.slot, self.count)
    }
}

fn opaque_i64(v: i64) -> i64 {
    v
}

fn opaque_u64(v: u64) -> u64 {
    v
}

fn opaque_i32(v: i32) -> i32 {
    v
}

fn main() {
    // a struct literal in `map` names the element type
    let row = Vec::<Vec<i8>>::new()
        .into_iter()
        .map(|_| Row {
            slot: None::<u16>,
            count: opaque_i64(0),
        })
        .nth(4)
        .unwrap_or_default();
    println!("row: {row:?} {}", row.describe());

    // a range source
    let (total, label, level) = (-1i64..-1i64)
        .map(|_| (opaque_u64(0), String::new(), opaque_i32(0)))
        .next_back()
        .unwrap_or_default();
    println!("tuple: {total:?} {label:?} {level:?}");
    println!("as_i64: {}", i64::from(level));

    let filled = (1i64..4i64)
        .map(|x| (x, x * 2))
        .next_back()
        .unwrap_or_default();
    println!("filled: {filled:?}");
}
