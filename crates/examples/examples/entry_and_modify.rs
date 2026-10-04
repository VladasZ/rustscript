#!/usr/bin/env rust

//! The closure of `and_modify` gets a `&mut` to the value in the map, so a write through it
//! lands there, a scalar included.

use std::collections::{BTreeMap, HashMap};

fn main() {
    let mut counts: BTreeMap<&str, i32> = BTreeMap::new();
    for word in ["a", "b", "a", "a"] {
        counts
            .entry(word)
            .and_modify(|count| *count += 1)
            .or_insert(1);
    }
    println!("{counts:?}");

    let mut lists: HashMap<&str, Vec<u8>> = HashMap::new();
    lists
        .entry("k")
        .and_modify(|list| list.push(1))
        .or_insert_with(|| vec![9]);
    lists
        .entry("k")
        .and_modify(|list| list.push(2))
        .or_insert_with(|| vec![8]);
    lists
        .entry("k")
        .and_modify(|list| {
            list.pop();
        })
        .or_default();
    println!("{lists:?}");

    let mut names: BTreeMap<u8, String> = BTreeMap::new();
    names
        .entry(1)
        .and_modify(|name| name.push('x'))
        .or_insert_with(|| "n".to_string());
    names
        .entry(1)
        .and_modify(|name| name.push('x'))
        .or_insert_with(|| "m".to_string());
    names
        .entry(1)
        .and_modify(|name| *name = name.to_uppercase());
    names.entry(2).and_modify(|name| name.push('y'));
    println!("{names:?}");
}
