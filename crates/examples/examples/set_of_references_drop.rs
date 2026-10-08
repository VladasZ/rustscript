#!/usr/bin/env rust

// A set collected from `iter()` holds references. It drops nothing, not for a repeated item
// and not when it goes away.

use std::collections::{BTreeSet, HashSet};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Lease(i64);

impl Drop for Lease {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn leases(ids: &[i64]) -> Vec<Lease> {
    ids.iter().map(|id| Lease(*id)).collect()
}

fn main() {
    println!("{:?}", leases(&[3, 1]).iter().collect::<BTreeSet<_>>());
    println!("--");

    let leases = leases(&[5, 4, 5]);
    let sorted = leases.iter().collect::<BTreeSet<_>>();
    println!("{} {:?}", sorted.len(), sorted.first());
    for lease in &sorted {
        print!("{} ", lease.0);
    }
    println!();
    let hashed = leases.iter().collect::<HashSet<_>>();
    println!("{}", hashed.len());
    drop(sorted);
    drop(hashed);
    println!("end");
}
