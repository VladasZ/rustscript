#!/usr/bin/env rust


use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
struct Manifest {
    deps: BTreeMap<String, String>,
    tags: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Priority {
    High,
    Low(u8),
}

fn main() {
    // insert keeps key order, not insertion order
    let mut ranked: BTreeMap<i32, &str> = BTreeMap::new();
    for (key, value) in [(5, "a"), (-2, "b"), (3, "c"), (9, "d")] {
        ranked.insert(key, value);
    }
    println!("{ranked:?} {:?}", ranked.keys().collect::<Vec<_>>());
    for (key, value) in &ranked {
        print!("{key}{value} ");
    }
    println!();
    println!(
        "{:?} {:?}",
        ranked.first_key_value(),
        ranked.last_key_value()
    );
    println!("{:?}", ranked.range(..4).collect::<Vec<_>>());
    println!(
        "{:?}",
        ranked.range(3..=5).map(|(key, _)| *key).collect::<Vec<_>>()
    );
    println!("{:?}", ranked.range(4..).rev().collect::<Vec<_>>());
    println!(
        "{:?} {:?} {ranked:?}",
        ranked.pop_first(),
        ranked.pop_last()
    );

    // collect, from and default all build a sorted collection
    let pairs = [(5, 'a'), (3, 'b'), (4, 'c')];
    let by_annotation: BTreeMap<i32, char> = pairs.iter().copied().collect();
    let by_turbofish = pairs.iter().copied().collect::<BTreeMap<_, _>>();
    let keys: BTreeSet<i32> = pairs.iter().map(|pair| pair.0).collect();
    #[allow(clippy::default_trait_access)]
    let empty: BTreeMap<u64, Vec<i32>> = Default::default();
    let wide = BTreeMap::from([(u64::MAX, 1), (0, 2)]);
    println!(
        "{by_annotation:?} {by_turbofish:?} {keys:?} {empty:?} {wide:?} {:?}",
        BTreeSet::from([3, 1, 2])
    );

    // or_default, string range bounds
    let mut words: BTreeMap<String, usize> = BTreeMap::new();
    for word in "b a c a b a".split(' ') {
        *words.entry(word.to_string()).or_default() += 1;
    }
    let from_a = words
        .range("a".to_string().."c".to_string())
        .collect::<Vec<_>>();
    println!("{words:?} {from_a:?}");

    // set ends, set operations come out ascending
    let mut letters: BTreeSet<char> = "hello world"
        .chars()
        .filter(|c| c.is_alphabetic())
        .collect();
    println!("{letters:?} {:?} {:?}", letters.first(), letters.last());
    println!(
        "{:?} {:?}",
        letters.pop_first(),
        letters.range('e'..'p').collect::<String>()
    );
    let left: BTreeSet<i32> = [5, 1, 3].into_iter().collect();
    let right: BTreeSet<i32> = [4, 3, 0].into_iter().collect();
    println!("{:?}", left.union(&right).collect::<Vec<_>>());
    println!("{:?}", left.intersection(&right).collect::<Vec<_>>());
    println!("{:?}", right.difference(&left).collect::<Vec<_>>());
    println!(
        "{:?}",
        left.symmetric_difference(&right)
            .copied()
            .collect::<BTreeSet<_>>()
    );

    // tuple and derived Ord keys
    let mut tuples = BTreeMap::new();
    tuples.insert((2, "x"), 1);
    tuples.insert((1, "z"), 2);
    tuples.insert((1, "a"), 3);
    println!("{tuples:?} {:?}", tuples.keys().next());
    let mut priorities = BTreeMap::new();
    priorities.insert(Priority::Low(3), "l3");
    priorities.insert(Priority::High, "h");
    priorities.insert(Priority::Low(1), "l1");
    println!("{priorities:?}");
    println!("{:#?}", BTreeSet::from([-5i64, 10, -100]));

    // serde reads into and writes from sorted collections
    let manifest: Manifest =
        serde_json::from_str(r#"{"deps":{"z":"1","a":"2"},"tags":["y","b"]}"#).unwrap();
    println!("{manifest:?}");
    println!("{}", serde_json::to_string(&manifest).unwrap());

    // std checks range bounds only on a map with entries
    let none: BTreeMap<i32, i32> = BTreeMap::new();
    let (high, low) = (5, 3);
    println!("{}", none.range(high..low).count());
}
