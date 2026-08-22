#!/usr/bin/env rust

//! Sequences order lexicographically. Comparing 2 vecs once aborted with
//! "cannot compare Vec and Vec", and `a.max(b)` was read as the iterator
//! reduction instead of `Ord::max`.

fn main() {
    println!("first differs: {:?}", vec![1, 2] < vec![1, 3]);
    println!("prefix first:  {:?}", vec![1, 2] < vec![1, 2, 0]);
    println!("head decides:  {:?}", vec![2] > vec![1, 9, 9]);
    println!("equal vecs:    {:?}", vec![1, 2] == vec![1, 2]);
    println!("tuple order:   {:?}", (1, "b") > (1, "a"));
    println!("string elems:  {:?}", vec!["a", "b"] < vec!["a", "c"]);

    // With an argument these are `Ord`.
    println!("ord max:       {:?}", vec![1, 2].max(vec![1, 3]));
    println!("ord min:       {:?}", vec![1, 2].min(vec![1, 3]));

    // Without one they are the reduction.
    println!("iter max:      {:?}", [3, 1, 2].iter().max());
    println!("iter min:      {:?}", [3, 1, 2].iter().min());

    let mut nested = vec![vec![2, 1], vec![1, 9], vec![1, 2]];
    nested.sort();
    println!("sorted nested: {nested:?}");

    // None sorts before Some and Ok before Err.
    println!("none first:    {:?}", None::<i32> < Some(1));
    println!("some order:    {:?}", Some(1) < Some(2));
    let ok: Result<i32, i32> = Ok(1);
    let err: Result<i32, i32> = Err(0);
    println!("ok before err: {:?}", ok < err);
    let mut options = vec![Some(3), None, Some(1)];
    options.sort();
    println!("sorted options:{options:?}");
    println!("option max:    {:?}", Some(1).max(Some(2)));
}
