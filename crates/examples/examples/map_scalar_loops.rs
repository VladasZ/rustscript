#!/usr/bin/env rust

// Map probes and inserts inside scalar `for` plans, with a mid loop failover
// after a journaled insert.

use std::collections::HashMap;

fn counting(n: i64) -> (usize, i64) {
    // The hashmap_int shape.
    let mut counts: HashMap<i64, i64> = HashMap::new();
    let mut x: i64 = 12345;
    for _ in 0..n {
        x = x * 48271 % 2_147_483_647;
        let key = x % 64;
        let next = counts.get(&key).copied().unwrap_or(0) + 1;
        counts.insert(key, next);
    }
    let mut total = 0;
    for key in 0..64 {
        if let Some(bucket) = counts.get(&key) {
            total += *bucket;
        }
    }
    (counts.len(), total)
}

fn kept_old_values() -> i64 {
    // `insert` answering the old value.
    let mut m: HashMap<i64, i64> = HashMap::new();
    let mut reclaimed = 0;
    for k in 0..30 {
        let old = m.insert(k % 10, k);
        if let Some(previous) = old {
            reclaimed += previous;
        }
    }
    reclaimed
}

fn membership() -> i64 {
    let mut seen: HashMap<i64, i64> = HashMap::new();
    let mut repeats = 0;
    for k in 0..40 {
        let key = k * k % 17;
        if seen.contains_key(&key) {
            repeats += 1;
        }
        seen.insert(key, k);
    }
    repeats
}

fn failing_over() -> (usize, f64) {
    // The float valued `weights` map fails every iteration over after the
    // insert into `counts`. A missed undo would skew the sum.
    let mut weights: HashMap<i64, f64> = HashMap::new();
    for k in 0..20 {
        weights.insert(k, f64::from(u32::try_from(k).unwrap()) * 0.5);
    }
    let mut counts: HashMap<i64, i64> = HashMap::new();
    let mut sum = 0.0;
    for k in 0..20 {
        let old = counts.insert(k, k * 2);
        if let Some(previous) = old {
            sum += f64::from(u32::try_from(previous).unwrap());
        }
        if let Some(w) = weights.get(&k) {
            sum += *w;
        }
    }
    (counts.len(), sum)
}

fn width_tagged_keys() -> i64 {
    // u32 keys.
    let mut m: HashMap<u32, i64> = HashMap::new();
    for k in 0u32..50 {
        let key = k % 7;
        let next = m.get(&key).copied().unwrap_or(1) * 2 % 1009;
        m.insert(key, next);
    }
    let mut folded = 0;
    for key in 0u32..7 {
        folded += m.get(&key).copied().unwrap_or(0);
    }
    folded
}

fn main() {
    let (keys, total) = counting(5000);
    println!("counting keys={keys} total={total}");
    println!("reclaimed={}", kept_old_values());
    println!("repeats={}", membership());
    let (entries, sum) = failing_over();
    println!("failover entries={entries} sum={sum}");
    println!("folded={}", width_tagged_keys());
}
