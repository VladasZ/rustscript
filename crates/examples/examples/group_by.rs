#!/usr/bin/env rustscript

use std::collections::HashMap;

fn first_char(s: &str) -> char {
    for c in s.chars() {
        return c;
    }
    ' '
}

fn main() {
    let words = vec!["apple", "banana", "cherry", "avocado", "blueberry", "citron"];
    let mut groups: HashMap<char, Vec<String>> = HashMap::new();
    for w in words {
        let key = first_char(w);
        let mut list = groups.get(&key).cloned().unwrap_or(vec![]);
        list.push(w.to_string());
        groups.insert(key, list);
    }
    let mut keys: Vec<char> = groups.keys().cloned().collect();
    keys.sort();
    for k in keys {
        let list = groups.get(&k).cloned().unwrap_or(vec![]);
        println!("{k}: {:?}", list);
    }
}
