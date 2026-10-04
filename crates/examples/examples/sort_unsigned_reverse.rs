#!/usr/bin/env rust

//! A sort compares the values themselves, so a `u64` past `i64::MAX` and a `Reverse` key order
//! like real Rust. `Reverse`, `iter::once` and `iter::repeat` keep the width of what they hold.
//! A map with no written value type takes it from `or_insert`.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap};
use std::iter::{once, repeat};

struct T(u8);

fn main() {
    let half = 9_223_372_036_854_775_808u64;
    let mut wide = vec![half, u64::MAX, 5];
    wide.sort_by_key(|value| Reverse(*value));
    println!("{wide:?}");
    wide.sort_by_key(|value| *value);
    println!(
        "{wide:?} {:?}",
        wide.iter().max_by_key(|value| Reverse(**value))
    );
    wide.sort_by(|left, right| right.cmp(left));
    println!("{wide:?} {}", Reverse(half) < Reverse(u64::MAX));

    let mut reversed = vec![Reverse(half), Reverse(u64::MAX), Reverse(5)];
    reversed.sort();
    println!("{reversed:?}");
    let mut pairs = vec![(2u8, "b"), (1, "z"), (2, "a")];
    pairs.sort_unstable();
    pairs.sort_by_key(|pair| Reverse(pair.1));
    println!("{pairs:?}");

    println!("{:?} {:?}", once(u64::MAX).next(), repeat(200u8).nth(3));
    println!(
        "{:?}",
        once(250u8).map(|value| value.checked_add(10)).next()
    );

    let mut counts = BTreeMap::new();
    for value in [1u8, 2, 1] {
        *counts.entry(value).or_insert(0usize) += 1;
    }
    let mut made = HashMap::new();
    for name in ["a", "a"] {
        *made.entry(name).or_insert_with(|| 10u16) += 1;
    }
    println!("{counts:?} {made:?} {}", !counts[&1]);

    let wrapped: Vec<u8> = (1..4).map(T).map(|item| item.0).collect();
    println!("{wrapped:?}");
}
