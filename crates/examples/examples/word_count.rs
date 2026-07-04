#!/usr/bin/env rustscript

use std::collections::HashMap;

fn main() {
    let text = "the quick brown fox the lazy dog the fox";
    let mut counts: HashMap<String, i64> = HashMap::new();
    for word in text.split(" ") {
        let n = counts.get(word).cloned().unwrap_or(0);
        counts.insert(word.to_string(), n + 1);
    }
    let mut pairs: Vec<(String, i64)> = counts.into_iter().collect();
    pairs.sort_by_key(|p| -p.1);
    for (word, n) in pairs {
        println!("{n} {word}");
    }
}
