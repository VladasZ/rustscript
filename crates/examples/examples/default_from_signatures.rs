#!/usr/bin/env rust

//! The type behind a bare `sum` or `unwrap_or_default` can come from a signature, a constructor or a
//! cast. A float `sum` of nothing is `-0.0`, and defaults behind `Ok::<T, E>`, a map `get` or a
//! tuple closure must have the right type.

use std::collections::{HashMap, HashSet};

/// std sums floats from `-0.0`
fn empty_float_sum(values: Vec<f64>) -> f64 {
    values.into_iter().map(|value: f64| value * 2.0).sum()
}

fn empty_product(values: Vec<u8>) -> u8 {
    values.into_iter().product()
}

fn first_or_default(values: Vec<i16>) -> i16 {
    values.first().copied().unwrap_or_default()
}

/// A generic helper returns the type its arguments state.
fn pick<T: Clone + std::fmt::Debug>(a: T, b: T, first: bool) -> T {
    if first { a } else { b }
}

fn largest_pair(values: Vec<u8>) -> (u8, bool) {
    values
        .into_iter()
        .map(|v: u8| (v, v > 3))
        .max()
        .unwrap_or_default()
}

fn main() {
    let empty: Vec<f64> = Vec::new();
    println!("empty sum: {:?}", empty_float_sum(empty));
    println!("empty sum padded: {:#06?}", empty_float_sum(Vec::new()));
    println!("empty product: {}", empty_product(Vec::new()));
    println!("first: {:?}", first_or_default(Vec::new()));
    println!("pair: {:?}", largest_pair(Vec::new()));
    println!("pair present: {:?}", largest_pair(vec![2, 9, 4]));

    // runtime false, so nothing folds
    let flag = std::env::args().count() > 1000;

    // the `Ok` turbofish names the map, so a missed `get` defaults to its value type
    let table = if flag {
        Ok::<HashMap<u32, (u8,)>, String>(HashMap::new())
    } else {
        Err(String::from("no table"))
    }
    .unwrap_or_default()
    .get(&7)
    .copied()
    .unwrap_or_default();
    println!("result map miss: {table:?}");

    // the `Err` turbofish states the map when the `Ok` side doesn't
    let scale = if flag {
        Ok(std::iter::empty().collect())
    } else {
        Err::<HashMap<String, i16>, String>(String::from("no table"))
    }
    .unwrap_or_default()
    .get("k")
    .copied()
    .unwrap_or_default();
    println!("err map miss: {}", scale * 3);

    // the closure's tuple of a cast and a suffixed literal is the only place the item type is written
    let total = std::env::args().count();
    let pair = Vec::<u32>::new()
        .into_iter()
        .map(|_: u32| (total as u64, 2i16))
        .max()
        .unwrap_or_default();
    println!("closure tuple miss: {pair:?}");

    // through a `collect` turbofish on a set
    let largest = Vec::<(i32, bool)>::new()
        .into_iter()
        .collect::<HashSet<(i32, bool)>>()
        .into_iter()
        .max()
        .unwrap_or_default();
    println!("set max miss: {largest:?}");

    // a qualified `default` and a match arm both state the nested vec
    let mut rows = <Vec<Vec<f64>>>::default().concat();
    for row in &mut rows {
        *row += 1.0;
    }
    println!("default concat: {rows:?}");
    let picked = match Some(1u8) {
        Some(_) => Vec::<Vec<u64>>::new(),
        None => vec![vec![1]],
    }
    .concat();
    println!("match concat: {picked:?}");

    // `Some` of a tuple states it through its items
    let (count, index, on) = Some((total as u64, total, !flag))
        .filter(|(seen, _, _)| flag && *seen > 0)
        .unwrap_or_default();
    println!("some tuple: {count} {index} {on}");

    // a `map` closure building a `vec!` of a typed local
    let seed: Vec<u8> = vec![1, 2];
    let rows = Vec::<u8>::new()
        .into_iter()
        .map(|_: u8| vec![seed.clone()])
        .nth(4)
        .unwrap_or_default()
        .into_iter()
        .skip(1)
        .collect::<Vec<Vec<u8>>>();
    println!("nth miss: {rows:?}");

    later_misses(flag);
}

/// Types from a local, a block, a generic helper or a literal.
fn later_misses(flag: bool) {
    // a tuple typed local states the element of the vec around it
    let mut label: (String,) = (String::from("start"),);
    label = [label.clone()].get(5).cloned().unwrap_or_default();
    println!("tuple miss: {label:?}");

    // 2 blocks reuse 1 local name for different map types
    let outer = ({
        let mut table: HashMap<char, (Option<char>,)> = HashMap::new();
        table.insert('a', (None,));
        table
    })
    .get(
        &({
            let mut table: HashMap<i32, char> = HashMap::new();
            table.insert(2, 'Z');
            table
        })
        .get(&7)
        .copied()
        .unwrap_or_default(),
    )
    .copied()
    .unwrap_or_default();
    println!("shadowed blocks: {outer:?}");

    // a block's value typed by the turbofish constructor of its local
    let removed = ({
        let mut owned = HashMap::<usize, (char, usize, Option<i8>)>::new();
        owned.remove(&1)
    })
    .unwrap_or_default();
    println!("block local miss: {removed:?}");

    // `pick` returns its `T`, which `None::<u16>` states
    let ratio = pick(None::<u16>, Some(65_534u16), !flag).unwrap_or_default() / 7;
    println!("generic pick miss: {ratio}");

    // `Some(x)` and `Ok(x)` state the payload through `x`
    let absent = Some(5u16)
        .filter(|value| flag && *value > 0)
        .unwrap_or_default();
    let failed = if flag {
        Ok(9u16)
    } else {
        Err(String::from("no"))
    }
    .unwrap_or_default();
    println!("literal payloads: {absent} {failed}");
}
