#!/usr/bin/env rust

//! `max`, `min` and `clamp` of a type with a hand written `Ord` go by that `Ord`, as methods,
//! through `std::cmp` and through `Ord::max`. The `cmp` prints, so how often and in which order
//! std calls it is a part of the output.

use std::cmp::{Ordering, max, max_by_key, min, min_by_key};

/// A bigger number is a smaller priority.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Priority(i32, &'static str);

impl PartialOrd for Priority {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Priority {
    fn cmp(&self, other: &Self) -> Ordering {
        println!("cmp {} {}", self.1, other.1);
        other.0.cmp(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Plain(u8, char);

fn main() {
    let (low, mid, high) = (Priority(3, "low"), Priority(2, "mid"), Priority(1, "high"));
    println!("{:?}", low.clone().max(high.clone()));
    println!("{:?}", low.clone().min(high.clone()));
    println!("{:?}", mid.clone().clamp(low.clone(), high.clone()));
    println!(
        "{:?}",
        Priority(9, "under").clamp(low.clone(), high.clone())
    );
    println!("{:?}", Priority(0, "over").clamp(low.clone(), high.clone()));

    println!("{:?}", max(low.clone(), mid.clone()));
    println!("{:?}", min(low.clone(), mid.clone()));
    println!("{:?}", max(&mid, &high));
    println!("{:?}", Ord::max(low.clone(), high.clone()));
    println!("{:?}", Ord::min(low.clone(), high.clone()));
    println!("{}", max_by_key(1, 2, |n| Priority(*n, "key")));
    println!("{}", min_by_key(1, 2, |n| Priority(*n, "key")));

    // equal by the `Ord`, so `max` takes the second and `min` the first
    println!("{:?}", Priority(5, "first").max(Priority(5, "second")));
    println!("{:?}", Priority(5, "first").min(Priority(5, "second")));
    println!("{:?}", max(Priority(5, "first"), Priority(5, "second")));
    println!("{:?}", min(Priority(5, "first"), Priority(5, "second")));

    // a derived `Ord` and a tuple order by their parts
    println!("{:?}", Plain(2, 'b').clamp(Plain(1, 'z'), Plain(2, 'a')));
    println!("{:?}", Plain(2, 'b').max(Plain(2, 'c')));
    println!("{:?}", (4, "d").clamp((1, "a"), (3, "c")));
}
