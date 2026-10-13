#!/usr/bin/env rust

//! A 128 bit integer is a key of a map and of a set. A `u128` key sorts as unsigned, also
//! above `i128::MAX`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

fn count_twice(key: &i128) -> usize {
    let mut seen: HashMap<&i128, &i128> = HashMap::new();
    seen.insert(key, key);
    seen.insert(key, key);
    seen.len()
}

fn main() {
    let mut names: BTreeMap<u128, &str> = BTreeMap::new();
    names.insert(u128::MAX, "max");
    names.insert(1 << 100, "big");
    names.insert(0, "zero");
    names.insert(u128::MAX - 1, "below max");
    println!("{names:?}");
    println!("{:?} {:?}", names.first_key_value(), names.last_key_value());
    println!("{:?} {:?}", names.get(&(1 << 100)), names.get(&7));

    let signed: BTreeSet<i128> = [5, i128::MIN, -1, i128::MAX, 0].into_iter().collect();
    println!("{signed:?}");
    println!("{} {}", signed.contains(&i128::MIN), signed.contains(&6));

    let mut seen: HashSet<i128> = HashSet::new();
    println!("{} {}", seen.insert(-(1 << 90)), seen.insert(-(1 << 90)));
    println!("{} {}", seen.len(), seen.contains(&-(1 << 90)));

    let mut pairs: HashMap<(u128, u8), Option<i128>> = HashMap::new();
    pairs.insert((u128::MAX, 1), Some(-3));
    pairs.insert((u128::MAX, 2), None);
    *pairs.entry((u128::MAX, 1)).or_insert(None) = Some(4);
    let mut sorted: Vec<((u128, u8), Option<i128>)> = pairs.into_iter().collect();
    sorted.sort_unstable();
    println!("{sorted:?}");

    let by_option: BTreeSet<Option<u128>> = [Some(u128::MAX), None, Some(3)].into_iter().collect();
    println!("{by_option:?}");
    println!("{}", count_twice(&4));
}
