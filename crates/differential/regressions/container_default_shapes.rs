//! Container and element defaults in generated shapes. The interpreter once
//! answered an empty string for them and clamped a `u64` sum at `i64::MAX`.

use std::collections::{HashMap, HashSet};

fn opaque(v: u64) -> u64 {
    v
}

fn main() {
    let missing: HashMap<i64, i64> = None::<HashMap<i64, i64>>.unwrap_or_default();
    println!("{}", missing.len());

    let chained: HashMap<i64, i64> = None::<Vec<HashMap<i64, i64>>>
        .unwrap_or_default()
        .get(4)
        .cloned()
        .unwrap_or_default();
    println!("{}", chained.len());

    let empty_set: HashSet<i64> = None::<HashSet<i64>>.unwrap_or_default();
    println!("{}", empty_set.len());

    let sorted_default = ({
        let mut sorted = vec!['b', 'a'];
        sorted.sort();
        sorted
    })
    .get(9)
    .cloned()
    .unwrap_or_default();
    println!("{sorted_default:?}");

    let inverted: i64 = !HashMap::<i64, i64>::new()
        .get(&4i64)
        .cloned()
        .unwrap_or_default();
    println!("{inverted}");

    let big: u64 = opaque(17387282529756548797u64);
    let lazy: u64 = vec![1i64]
        .into_iter()
        .map(|_n| big.min(opaque(u64::MAX)))
        .sum::<u64>();
    println!("{}", lazy as i16);

    let mut source: HashSet<i64> = HashSet::new();
    source.insert(0);
    let eager = source
        .into_iter()
        .map(|_n: i64| big.min(opaque(u64::MAX)))
        .sum::<u64>();
    println!("{}", eager as i16);
    println!("{}", eager as i8);
    println!("{}", eager as u32);
}
