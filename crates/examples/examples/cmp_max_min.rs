#!/usr/bin/env rust

//! `std::cmp::max` and `min` as free functions, with their `_by` and `_by_key` forms. For equal
//! values `max` hands back the second argument and `min` the first. The argument that loses
//! drops inside the call.

use std::cmp::{max, max_by, max_by_key, min, min_by, min_by_key};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    major: u8,
    name: String,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Loud(u8);

impl Drop for Loud {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn version(major: u8, name: &str) -> Version {
    Version {
        major,
        name: name.to_string(),
    }
}

fn by_len(left: &&str, right: &&str) -> std::cmp::Ordering {
    println!("compare {left} {right}");
    left.len().cmp(&right.len())
}

fn main() {
    println!("{} {}", std::cmp::max(3, 9), std::cmp::min(3, 9));
    let (low, high) = (200u8, 250u8);
    println!("{} {}", max(low, high), min(low, high));
    // the width of the answer shows in `!x`
    println!("{} {}", !max(low, 7), !min(-1i64, 4));
    println!("{} {}", max(u64::MAX, 1), min(i128::MIN, 0));
    println!("{} {}", max('a', 'z'), min('a', 'z'));
    println!("{} {}", max("pear", "apple"), min("pear", "apple"));
    println!(
        "{} {}",
        max(String::from("b"), String::from("a")),
        min(String::from("b"), String::from("a"))
    );
    println!(
        "{:?} {:?}",
        max((1, "b"), (1, "a")),
        min((2, 'x'), (1, 'y'))
    );
    println!("{:?} {:?}", max(Some(3), None), min(Some(3), None));
    println!("{:?}", max(vec![1, 2], vec![1, 3]));
    println!("{:?}", max(version(1, "old"), version(2, "new")));
    println!("{:?}", min(version(1, "b"), version(1, "a")));

    let (first, second) = (version(3, "first"), version(3, "second"));
    let newest = max(&first, &second);
    println!("{newest:?}");
    println!("{}", [3u32, 8, 5].into_iter().fold(0, max));

    println!("{}", max_by("fig", "plum", by_len));
    println!("{}", min_by("fig", "plum", by_len));
    // equal by the closure, so `max_by` takes the second and `min_by` the first
    println!(
        "{} {}",
        max_by("ab", "cd", |a, b| a.len().cmp(&b.len())),
        min_by("ab", "cd", |a, b| a.len().cmp(&b.len()))
    );

    let far = max_by_key(-7i32, 5, |n| {
        println!("key of {n}");
        n.unsigned_abs()
    });
    let near = min_by_key(-7i32, 5, |n| n.unsigned_abs());
    println!("{far} {near}");
    println!(
        "{} {}",
        max_by_key(-5i32, 5, |n| n.unsigned_abs()),
        min_by_key(-5i32, 5, |n| n.unsigned_abs())
    );

    let kept = max(Loud(1), Loud(2));
    println!("kept {}", kept.0);
    let small = min_by_key(Loud(7), Loud(4), |loud| loud.0 % 4);
    println!("small {}", small.0);
    let (left, right) = (Loud(8), Loud(9));
    let borrowed = min(&left, &right);
    println!("borrowed {}", borrowed.0);
}
