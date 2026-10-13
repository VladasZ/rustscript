// A clone of a `BTreeMap` or a `BTreeSet` keeps its key order on the next insert. A `mut`
// closure parameter with no type is such a clone.

use std::collections::{BTreeMap, BTreeSet};

fn main() {
    let mut a = BTreeMap::new();
    a.insert(5, 1u8);
    let mut b = a.clone();
    b.insert(4, 1);
    b.insert(9, 1);
    b.insert(1, 1);
    println!("{b:?}");
    let c = [7i64, 3, 9].iter().fold(BTreeMap::new(), |mut m, n| {
        m.insert(*n, 1u8);
        m
    });
    println!("{c:?}");
    let d = [7i64, 3, 9].iter().fold(BTreeSet::new(), |mut s, n| {
        s.insert(*n);
        s
    });
    println!("{d:?}");
    let mut e = d.clone();
    e.insert(1);
    println!("{e:?} {:?}", e.first());
    let nested = vec![b.clone()];
    let mut inner = nested.clone();
    inner[0].insert(0, 0);
    println!("{inner:?}");
}
