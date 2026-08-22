#!/usr/bin/env rust

//! A clone is its own storage. A `&mut` write, an `as_mut` write, a `get_mut` write and a
//! `first_mut` write land in the original and never in a clone taken before them.

use std::collections::HashMap;

#[derive(Clone, Debug)]
struct Point {
    x: i32,
    tags: Vec<String>,
}

#[derive(Clone, Debug)]
struct Counter {
    n: i32,
}

fn bump(p: &mut Point) {
    p.x += 1;
    p.tags.push("b".to_string());
}

fn tick(c: &mut Counter) {
    c.n += 1;
}

fn main() {
    let mut p = Point {
        x: 0,
        tags: Vec::new(),
    };
    let before = p.clone();
    for _ in 0..3 {
        bump(&mut p);
    }
    println!("{p:?} {before:?}");

    let mut items = vec![Counter { n: 0 }];
    let snapshot = items.clone();
    tick(&mut items[0]);
    println!("{items:?} {snapshot:?}");

    let mut by_key = HashMap::new();
    by_key.insert(1, Counter { n: 0 });
    let keyed = by_key.clone();
    tick(by_key.get_mut(&1).unwrap());
    println!("{:?} {:?}", by_key[&1], keyed[&1]);

    let mut rows = vec![vec![1], vec![2]];
    let kept = rows.clone();
    rows.first_mut().unwrap().push(10);
    rows.last_mut().unwrap().push(20);
    rows.get_mut(0).unwrap().push(30);
    let lens: Vec<usize> = rows
        .iter_mut()
        .map(|row| {
            row.push(40);
            row.len()
        })
        .collect();
    for row in &mut rows {
        row.push(50);
    }
    println!("{rows:?} {kept:?} {lens:?}");

    let mut maybe = Some(vec![1]);
    let same = maybe.clone();
    maybe.as_mut().unwrap().push(2);
    if let Some(inner) = maybe.as_mut() {
        inner.push(3);
    }
    if let Some(inner) = &mut maybe {
        inner.push(4);
    }
    println!("{maybe:?} {same:?}");

    // a `let mut` of a `Copy` value is a copy, the source stays as it was
    let pair = (1, vec![9]);
    let mut other = pair.clone();
    other.0 = 2;
    other.1.push(8);
    println!("{pair:?} {other:?}");
}
