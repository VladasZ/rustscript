#!/usr/bin/env rust

// Mutation through `&mut` borrows in every binding shape: pattern payloads,
// mutable iteration, and the `mem` place functions. Writes through a borrow
// must land in the borrowed place.

use std::mem::take;

fn main() {
    let mut n = Some(5);
    if let Some(x) = &mut n {
        *x += 1;
    }
    println!("{n:?}");

    let mut opt = Some(vec![1]);
    if let Some(v) = &mut opt {
        v.push(2);
    }
    println!("{opt:?}");

    let mut pair = Some((1, String::from("a")));
    if let Some((num, text)) = &mut pair {
        *num += 1;
        text.push('!');
    }
    println!("{pair:?}");

    let mut vals = vec![Some(1), None, Some(3)];
    let mut skipped = 0;
    for v in &mut vals {
        if let Some(x) = v {
            *x *= 10;
        } else {
            skipped += 1;
        }
    }
    println!("{vals:?} {skipped}");

    let mut nums = vec![1, 2, 3];
    for x in &mut nums {
        *x += 100;
    }
    println!("{nums:?}");

    let mut a = vec![1];
    let mut b = vec![2];
    std::mem::swap(&mut a, &mut b);
    println!("{a:?} {b:?}");

    let mut s = String::from("gone");
    let old = take(&mut s);
    println!("{old} {s:?}");

    let prev = std::mem::replace(&mut a, vec![9]);
    println!("{prev:?} {a:?}");

    let mut swap_fields = (String::from("l"), String::from("r"));
    std::mem::swap(&mut swap_fields.0, &mut swap_fields.1);
    println!("{swap_fields:?}");

    let mut counts = std::collections::HashMap::new();
    counts.insert("k".to_string(), vec![1]);
    if let Some(list) = counts.get_mut("k") {
        list.push(2);
    }
    println!("{:?}", counts["k"]);

    let mut grid = vec![vec![1, 2], vec![3]];
    if let Some(row) = grid.first_mut() {
        row.push(99);
    }
    if let Some(row) = grid.last_mut() {
        row.clear();
    }
    println!("{grid:?}");
}
