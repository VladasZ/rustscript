#!/usr/bin/env rust

//! A `Vec::new()` with no written type takes its element type from what goes into it later.
//! When nothing goes in at run time the vec has no item to show its type, so `concat`, `join`,
//! `unwrap_or_default` and a trait impl on `Vec<f64>` go by that later use. Seeds 20732001332,
//! 20736206127 and 20737116691.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::env::args;

trait Describe {
    fn describe(&self) -> String;
}

impl Describe for Vec<f64> {
    fn describe(&self) -> String {
        format!("f64s {self:?}")
    }
}

impl Describe for Vec<u16> {
    fn describe(&self) -> String {
        format!("u16s {self:?}")
    }
}

fn main() {
    // runtime false, so every collection below stays empty
    let flag = args().count() > 1000;

    // `push` names the item
    let mut rows = Vec::new();
    if flag {
        rows.push(vec![1u8]);
    }
    println!("concat rows:   {:?}", rows.concat());
    println!("join rows:     {:?}", rows.join(&0));

    let mut words = Vec::new();
    if flag {
        words.push(String::from("a"));
    }
    println!("concat words:  {:?}", words.concat());
    println!("join words:    {:?}", words.join("-"));

    // the impl for `Vec<f64>`, not the one for `Vec<u16>`
    let mut floats = Vec::new();
    if flag {
        floats.push(1.5f64);
    }
    println!("describe:      {}", floats.describe());
    let total: f64 = floats.iter().sum();
    println!("sum:           {total:?}");

    // the default of a set is a set
    let mut sets = Vec::new();
    if flag {
        sets.push(HashSet::<char>::new());
    }
    let mut letters = sets.into_iter().next().unwrap_or_default();
    letters.insert('x');
    println!("set default:   {letters:?}");

    // filled by a `while let` over an iterator that ends at once
    let mut source = Vec::<Vec<i16>>::new().into_iter().peekable();
    let mut kept = Vec::new();
    while let Some(row) = source.next() {
        if source.peek().is_some() {
            kept.push(row);
        }
    }
    println!("while let:     {:?}", kept.concat());

    // `insert` and `extend`
    let mut inserted = Vec::new();
    if flag {
        inserted.insert(0, vec!['a']);
    }
    println!("insert:        {:?}", inserted.concat());
    let mut extended = Vec::new();
    if flag {
        extended.extend(vec![vec![1i16]]);
    }
    println!("extend:        {:?}", extended.concat());

    // a deque and a map
    let mut queue = VecDeque::new();
    if flag {
        queue.push_back(BTreeMap::<u8, u8>::new());
    }
    let mut front = queue.pop_front().unwrap_or_default();
    front.insert(1, 2);
    println!("deque default: {front:?}");
    let mut by_id = HashMap::new();
    if flag {
        by_id.insert(1u8, vec![(2u8, 0.5f32)]);
    }
    println!("map default:   {:?}", by_id.remove(&1).unwrap_or_default());

    // an assignment names the type of a `None`
    let mut found = None;
    if flag {
        found = Some(HashSet::<u8>::new());
    }
    let mut seen = found.unwrap_or_default();
    seen.insert(3);
    println!("assigned:      {seen:?}");

    // the rows of a vec of vecs join into one flat vec
    let nested = [vec![1u8, 2], vec![3], vec![]];
    println!("join one:      {:?}", nested.join(&9));
    println!("join slice:    {:?}", nested.join(&[7, 8][..]));
    let deep = [vec![vec![1u8]], vec![vec![2]]];
    let gap = vec![0u8];
    println!("join deep:     {:?}", deep.join(&gap));
}
