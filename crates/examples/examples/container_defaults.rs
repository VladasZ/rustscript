#!/usr/bin/env rust

//! Defaults built from container and element types the source states, the
//! shapes the differential campaign found answering an empty string instead.

use std::collections::{HashMap, HashSet};

fn lookup() -> Option<HashMap<i64, i64>> {
    None
}

fn main() {
    // A map default through the receiver's annotated `Option` payload.
    let found = lookup();
    let missing = found.unwrap_or_default();
    println!("{}", missing.len());

    // A map default through an annotated vec and an out-of-range get.
    let stack: Vec<HashMap<i64, i64>> = Vec::new();
    let chained = stack.get(4).cloned().unwrap_or_default();
    println!("{}", chained.len());

    // A set default through a `let` annotation.
    let sets: Vec<HashSet<i64>> = Vec::new();
    let empty_set: HashSet<i64> = sets.first().cloned().unwrap_or_default();
    println!("{}", empty_set.len());

    // A char default from a vec literal behind a block-local binding.
    let sorted_default = ({
        let mut sorted = vec!['b', 'a'];
        sorted.sort_unstable();
        sorted
    })
    .get(9)
    .copied()
    .unwrap_or_default();
    println!("{sorted_default:?}");

    // A char default through a mapping closure's stated element type.
    let mapped = (Vec::<i8>::new().into_iter().map(|_n: i8| 'a').max()).unwrap_or_default();
    println!("{:?}", mapped.to_ascii_uppercase());

    // An i64 default through `HashMap::get` on an empty map, then `!` on it.
    let key: i64 = 4;
    let inverted: i64 = !HashMap::<i64, i64>::new()
        .get(&key)
        .copied()
        .unwrap_or_default();
    println!("{inverted}");

    // A vec-of-strings default through a mapping closure, then methods on it.
    let empty_names: Option<String> = (HashSet::<i64>::new()
        .into_iter()
        .map(|_n: i64| vec![String::from("a"), String::from("b")])
        .max())
    .unwrap_or_default()
    .iter()
    .min()
    .cloned();
    println!("{empty_names:?}");

    // A map default from vec elements stated by block tails. The vec binding
    // carries no annotation, its elements state the type themselves.
    let mut maps = vec![
        ({
            let mut m: HashMap<i64, i64> = HashMap::new();
            m.insert(1, 10);
            m
        }),
        ({
            let mut m: HashMap<i64, i64> = HashMap::new();
            m.insert(2, 20);
            m
        }),
    ];
    maps.push(HashMap::new());
    let from_blocks = maps.get(7).cloned().unwrap_or_default();
    let total: i64 = from_blocks.into_values().sum::<i64>();
    println!("{total}");
}
