#!/usr/bin/env rust

// `std::cmp::Reverse` flips the order of whatever it wraps, on its own and inside a tuple key.

use std::cmp::Reverse;

struct Session {
    name: String,
    modified: i64,
}

fn main() {
    // newest first, the shape the session picker uses
    let mut sessions = vec![
        Session {
            name: "alpha".to_string(),
            modified: 30,
        },
        Session {
            name: "beta".to_string(),
            modified: 10,
        },
        Session {
            name: "gamma".to_string(),
            modified: 20,
        },
    ];
    sessions.sort_by_key(|s| std::cmp::Reverse(s.modified));
    for s in &sessions {
        println!("{} {}", s.name, s.modified);
    }

    let mut nums = vec![3, 1, 4, 1, 5, 9, 2, 6];
    nums.sort_by_key(|n| Reverse(*n));
    println!("descending {nums:?}");

    // count descending, then name ascending, the word_count shape
    let mut pairs = vec![("fox", 2), ("the", 3), ("dog", 1), ("ant", 3)];
    pairs.sort_by_key(|p| (Reverse(p.1), p.0));
    println!("pairs {pairs:?}");

    let mut words = vec!["pear", "apple", "plum", "fig"];
    words.sort_by_key(|w| Reverse(w.len()));
    println!("by len {words:?}");

    let shortest = words.iter().max_by_key(|w| Reverse(w.len()));
    println!("shortest {shortest:?}");
    let longest = words.iter().min_by_key(|w| Reverse(w.len()));
    println!("longest {longest:?}");

    // a wrapped value compares flipped, prints as the tuple struct, and hands its payload back
    let small = Reverse(1);
    let big = Reverse(2);
    println!("flipped {} {}", small > big, small.0 < big.0);
    println!("debug {small:?}");
    println!("cmp {:?}", small.cmp(&big));

    let mut wrapped = vec![Reverse(5), Reverse(1), Reverse(3)];
    wrapped.sort();
    let plain: Vec<i64> = wrapped.into_iter().map(|r| r.0).collect();
    println!("sorted wrapped {plain:?}");

    let mut names = vec!["cd".to_string(), "ab".to_string(), "zz".to_string()];
    names.sort_by_key(|n| Reverse(n.clone()));
    println!("strings {names:?}");
}
