#!/usr/bin/env rust

use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

// the result holds references into `items`, so the caller drops nothing of them
fn large(items: &[T]) -> Vec<&T> {
    items.iter().filter(|t| t.0 > 4).collect()
}

fn first_large(items: &[T]) -> Option<&T> {
    items.iter().find(|t| t.0 > 4)
}

fn main() {
    let items = vec![T(5), T(4), T(5)];
    let fallback = T(0);

    // a map whose keys or values are references, a repeated key drops nothing
    let by_item = items.iter().map(|t| (t, t.0)).collect::<BTreeMap<_, _>>();
    println!("a {by_item:?}");
    let by_id: BTreeMap<i64, &T> = items.iter().map(|t| (t.0, t)).collect();
    println!("b {by_id:?}");
    let pairs: Vec<(&T, usize)> = items.iter().zip(1usize..).collect();
    println!("c {pairs:?}");
    let numbered = items
        .iter()
        .enumerate()
        .map(|(i, t)| (t, i))
        .collect::<HashMap<_, _>>();
    println!("d {}", numbered.len());

    // references kept by `insert`, `push` and `entry`
    let mut seen = BTreeSet::new();
    let mut order = Vec::new();
    let mut counts: BTreeMap<&T, usize> = BTreeMap::new();
    for t in &items {
        if seen.insert(t) {
            order.push(t);
        }
        *counts.entry(t).or_insert(0) += 1;
    }
    println!("e {seen:?} {order:?} {counts:?}");
    let folded = items.iter().fold(BTreeMap::new(), |mut map, t| {
        map.insert(t, t.0 * 2);
        map
    });
    println!("f {folded:?}");
    let mut extended = BTreeMap::new();
    extended.extend(items.iter().map(|t| (t, 1u8)));
    println!("g {extended:?}");

    // a fallback passed by reference stays with its owner
    let found = by_id.get(&4).copied().unwrap_or(&fallback);
    let missing = by_id.get(&9).copied().unwrap_or(&fallback);
    println!("h {found:?} {missing:?}");
    let picked: Vec<Option<&T>> = items.iter().map(|t| (t.0 > 4).then_some(t)).collect();
    println!("i {picked:?}");

    // functions that hand references out, and a clone of what they hand out
    println!("j {:?} {:?}", large(&items), first_large(&items));
    let copy = first_large(&items).cloned();
    println!("k {copy:?}");
    let built = items.first().map(|t| vec![t.clone()]);
    println!("l {built:?}");

    // slices held by bindings go into an array as references
    let (head, tail) = items.split_at(1);
    let swapped = [tail, head].concat();
    println!("m {swapped:?}");

    // a clone of a `BTreeMap` stays in key order
    let mut copied = by_id.clone();
    copied.insert(1, &fallback);
    println!("n {copied:?}");

    // `ref mut` in a `let else` writes into the place
    let mut slot = Some(3usize);
    let Some(ref mut inner) = slot else {
        return;
    };
    *inner += 1;
    println!("o {slot:?}");
    println!("end");
}
