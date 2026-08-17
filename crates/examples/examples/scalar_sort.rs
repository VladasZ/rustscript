#!/usr/bin/env rust

// Comparator sorts over plain ints run through the scalar specialization,
// which sorts unboxed and must stay byte for byte equal to real Rust. The
// later cases fall outside the int-only subset on purpose, so the generic
// path keeps covering captures in cells, strings, and floats.

use std::cmp::Ordering;

fn main() {
    // The bucket shape from the sort benchmark: group first, value second.
    let mut buckets = vec![503, 1503, 7, 2007, 12, 1012, 999, 1999, 503];
    buckets.sort_by(|a, b| {
        if a % 1000 == b % 1000 {
            a.cmp(b)
        } else {
            (a % 1000).cmp(&(b % 1000))
        }
    });
    println!("buckets {buckets:?}");

    // Reverse order, and extreme values with no arithmetic on them.
    let mut extremes = vec![i64::MAX, 0, i64::MIN, 42, -7];
    extremes.sort_by(|a, b| b.cmp(a));
    println!("reverse {extremes:?}");

    // Ordering literals picked by branches.
    let mut branchy = vec![9, 1, 8, 2, 7, 3];
    branchy.sort_by(|a, b| {
        if a < b {
            return Ordering::Less;
        }
        if a > b {
            return Ordering::Greater;
        }
        Ordering::Equal
    });
    println!("literals {branchy:?}");

    // A captured int is snapshotted into the plan as a constant.
    let pivot = 5;
    let mut grouped = vec![9, 1, 6, 3, 5, 8, 2];
    grouped.sort_by(|a, b| {
        if a % pivot == b % pivot {
            a.cmp(b)
        } else {
            (a % pivot).cmp(&(b % pivot))
        }
    });
    println!("captured {grouped:?}");

    // A mutable capture must stay on the generic path, which reads the cell
    // per call.
    let mut calls = 0;
    let mut counted = vec![4, 2, 9, 1];
    counted.sort_by(|a, b| {
        calls += 1;
        a.cmp(b)
    });
    println!("counted {counted:?} calls={}", calls > 0);

    // Non-int elements always take the generic path.
    let mut words = vec!["pear", "apple", "plum", "fig"];
    words.sort_by(|a, b| a.len().cmp(&b.len()).then(a.cmp(b)));
    println!("strs {words:?}");

    let mut floats = vec![2.5, -1.0, 0.5, 9.25];
    floats.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("floats {floats:?}");

    // The rest of the Ordering surface: then_with, reverse, and the tests.
    let left = 3;
    let right = 3;
    let tie = left.cmp(&right).then_with(|| 10.cmp(&2));
    println!(
        "ordering {tie:?} {:?} {} {}",
        tie.reverse(),
        tie.is_gt(),
        left.cmp(&right).is_eq()
    );
}
