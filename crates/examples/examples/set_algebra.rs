#!/usr/bin/env rust

// The two set methods of `HashSet`: the relations and the combinations,
// every combination sorted before printing because the order is unpromised.

use std::collections::HashSet;

fn sorted(items: impl Iterator<Item = i32>) -> Vec<i32> {
    let mut out: Vec<i32> = items.collect();
    out.sort_unstable();
    out
}

fn main() {
    let a: HashSet<i32> = [1, 2, 3, 4].into_iter().collect();
    let b: HashSet<i32> = [3, 4, 5].into_iter().collect();
    let c: HashSet<i32> = [3, 4].into_iter().collect();
    println!("{:?}", sorted(a.union(&b).copied()));
    println!("{:?}", sorted(a.intersection(&b).copied()));
    println!("{:?}", sorted(a.difference(&b).copied()));
    println!("{:?}", sorted(b.difference(&a).copied()));
    println!("{:?}", sorted(a.symmetric_difference(&b).copied()));
    println!(
        "{} {} {}",
        c.is_subset(&a),
        a.is_subset(&c),
        a.is_superset(&c)
    );
    println!(
        "{} {}",
        a.is_disjoint(&b),
        c.is_disjoint(&HashSet::<i32>::new())
    );
}
