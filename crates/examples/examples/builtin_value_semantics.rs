#!/usr/bin/env rust

//! Builtin values behaving like std's. Each line here is a divergence the
//! differential campaign found: two empty sets comparing unequal, a
//! `String::clear` answered by the colored crate's `clear` and leaving the
//! text in place, `concat` of no nested vecs answering a string, the zero
//! flag lost in front of a positional width, and a bare literal under a
//! `let` annotation living as an i64 until the binding retagged it.

use std::collections::HashSet;

fn wipe(buffer: &mut String) {
    buffer.clear();
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Reading {
    tick: i64,
    label: String,
}

/// A `let` annotation types the bare literals inside its init, through the
/// branches of an `if`, the elements of a `vec!`, an array, a tuple, and the
/// payload of a `Some`, so each value wraps at its own width.
fn typed_literals(flag: bool) {
    // The annotation types the literals inside the branches, so the
    // negation runs at i32 and the bytes wrap at u8.
    let picked: i32 = -(if flag { 7 } else { 2_000_000_000 });
    println!("picked: {picked}");
    let bytes: Vec<u8> = vec![250, 10, if flag { 1 } else { 255 }];
    let bumped: Vec<u8> = bytes.iter().map(|b| b.wrapping_add(10)).collect();
    println!("wrapped bytes: {bumped:?}");
    let pair: (u8, i16) = (200, -300);
    println!(
        "pair: {:?}",
        (pair.0.wrapping_mul(2), pair.1.wrapping_mul(200))
    );
    let grid: Vec<Vec<u16>> = vec![vec![65_000, 1], vec![]];
    println!("grid: {:?}", grid[0][0].wrapping_add(1_000));
    let maybe: Option<u8> = Some(255);
    println!("maybe: {:?}", maybe.map(|v| v.wrapping_add(1)));
    let arr: [i8; 2] = [-128, 127];
    println!(
        "arr: {:?}",
        arr.iter().map(|v| v.wrapping_sub(1)).collect::<Vec<i8>>()
    );
}

fn main() {
    let flag = std::env::args().count() > 1000;

    let left: HashSet<usize> = HashSet::new();
    let right: HashSet<usize> = HashSet::new();
    println!("empty sets equal: {}", left == right);
    let chosen: Vec<f32> = if left == right { vec![0.5] } else { Vec::new() };
    println!("chosen: {chosen:?}");

    let mut text = String::from("TRUE");
    text.clear();
    println!("cleared local: {text:?} len {}", text.len());
    let mut buffer = String::from("pending");
    wipe(&mut buffer);
    println!("cleared through ref: {buffer:?}");
    let mut rows = vec![String::from("a"), String::from("b")];
    if let Some(last) = rows.last_mut() {
        last.clear();
    }
    println!("cleared element: {rows:?}");

    let nested: Vec<Vec<Option<usize>>> = Vec::new();
    let flattened = nested.concat();
    println!("empty concat: {flattened:?} {}", flattened.len());
    println!(
        "compare: {}",
        vec![None::<usize>] <= Vec::<Vec<Option<usize>>>::new().concat()
    );
    let words: Vec<String> = Vec::new();
    println!("empty string concat: {:?}", words.concat());

    println!("zero flag width: {0:#01$x}", 0i64, 9usize);
    // A print argument made only of bare literals is the `i32` rustc falls
    // back to, so a negative one shows eight hex digits.
    println!(
        "bare literal hex: {:#x} {:b}",
        if flag { 0 } else { -1 },
        -(2 + 3)
    );
    // A suffixed literal in one branch types the bare one in the other.
    let shown = if flag { 0i32 } else { -1 };
    println!(
        "sibling typed: {shown:#x} {:#x}",
        if flag { 0u8 } else { 255 }
    );
    println!(
        "zero flag named: {value:#0width$b}",
        value = 5u8,
        width = 10
    );

    typed_literals(flag);

    // The annotated sum types the closure body too, so its literal is an
    // i32 and wraps the way an i32 does.
    let folded: i32 = vec![1u8, 2]
        .into_iter()
        .map(|_| if flag { 7 } else { 1_000_000_000 })
        .sum();
    println!("folded: {}", folded.wrapping_add(2_000_000_000));

    // A range pattern bound past `i64::MAX` still matches.
    let big: u64 = 9_223_372_036_854_775_808;
    let bucket = match big {
        0..=9_223_372_036_854_775_807 => "low",
        9_223_372_036_854_775_808..=u64::MAX => "high",
    };
    println!("bucket: {bucket}");

    // A derived `Ord` orders by the fields in declaration order.
    let readings = [
        Reading {
            tick: 9_223_372_036_854_775_806,
            label: String::from("b"),
        },
        Reading {
            tick: 9_223_372_036_854_775_807,
            label: String::from("a"),
        },
        Reading {
            tick: 9_223_372_036_854_775_807,
            label: String::from("c"),
        },
    ];
    println!("max: {:?}", readings.iter().max());
    println!("min: {:?}", readings.iter().min());
    println!("ordered: {}", readings[0] < readings[1]);

    // `sort` orders enums by variant and then by payload, not by their
    // printed form.
    let mut options = vec![Some(-1i16), None, Some(-32767i16), Some(7)];
    options.sort();
    println!("sorted options: {options:?}");
    let mut results = vec![Err::<u8, u8>(1), Ok(9), Ok(2)];
    results.sort();
    println!("sorted results: {results:?}");

    // A `move` closure owns a copy of the counter, so the outer binding
    // keeps its value after the closure counts down.
    let mut remaining: u64 = 10;
    let mut countdown = move || -> u64 {
        remaining -= 2;
        remaining
    };
    println!("countdown: {} {}", countdown(), countdown());
    println!("remaining: {remaining}");
    let mut shared: u64 = 10;
    let mut drain = || -> u64 {
        shared -= 2;
        shared
    };
    println!("drain: {}", drain());
    println!("shared: {shared}");
}
