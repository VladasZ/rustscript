#!/usr/bin/env rust


use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Default)]
struct Stats {
    count: u32,
    total: i64,
}

fn sorted<K: Ord + Clone, V: Clone>(map: &HashMap<K, V>) -> Vec<(K, V)> {
    let mut pairs: Vec<(K, V)> = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs
}

fn main() {
    // the default is built from the map's value type
    let mut counts: HashMap<String, usize> = HashMap::new();
    for word in "b a c a b a".split(' ') {
        *counts.entry(word.to_string()).or_default() += 1;
    }
    println!("{:?}", sorted(&counts));

    let mut small: HashMap<char, u8> = HashMap::new();
    for c in "hello".chars() {
        *small.entry(c).or_default() += 1;
    }
    println!("{:?}", sorted(&small));

    let mut text: HashMap<&str, String> = HashMap::new();
    text.entry("a").or_default().push_str("one");
    text.entry("a").or_default().push('!');
    println!("{:?}", sorted(&text));

    let mut buckets: HashMap<bool, Vec<i32>> = HashMap::new();
    for n in 1..=6 {
        buckets.entry(n % 2 == 0).or_default().push(n);
    }
    println!("{:?}", sorted(&buckets));

    let mut groups: HashMap<u8, HashSet<char>> = HashMap::new();
    groups.entry(1).or_default().insert('x');
    println!("{}", groups[&1].contains(&'x'));

    let mut per_key: BTreeMap<&str, Stats> = BTreeMap::new();
    for (key, value) in [("a", 3), ("b", -1), ("a", 4)] {
        let stats = per_key.entry(key).or_default();
        stats.count += 1;
        stats.total += value;
    }
    println!("{per_key:?}");

    let mut floats: HashMap<i32, f64> = HashMap::new();
    *floats.entry(1).or_default() += 0.5;
    println!("{:?}", floats.get(&1));
}
