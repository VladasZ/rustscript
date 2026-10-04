#!/usr/bin/env rust

//! `drain` on a map or a set hands every entry out and leaves the container empty.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

fn main() {
    let mut set: HashSet<u8> = HashSet::from([3, 1, 2]);
    let mut drained: Vec<u8> = set.drain().collect();
    drained.sort_unstable();
    println!("{drained:?} {} {}", set.len(), set.is_empty());
    set.insert(9);
    println!("{set:?}");

    let mut map: HashMap<&str, i32> = HashMap::from([("a", 1), ("b", 2)]);
    let mut pairs: Vec<(&str, i32)> = map.drain().collect();
    pairs.sort_unstable();
    println!("{pairs:?} {} {:?}", map.len(), map.get("a"));
    map.insert("c", 3);
    println!("{map:?}");

    let mut ordered: BTreeMap<u8, char> = BTreeMap::from([(2, 'b'), (1, 'a')]);
    let first = ordered.pop_first();
    println!("{first:?} {ordered:?}");
    let mut letters: BTreeSet<char> = BTreeSet::from(['z', 'y']);
    letters.clear();
    println!("{}", letters.len());
}
