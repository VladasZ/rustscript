#!/usr/bin/env rust

//! `scan`, `map_while`, `cycle`, `iter::repeat` and `iter::repeat_n` are lazy and drop what
//! they hold where real Rust does. `len` reads the exact size of an iterator.

use std::iter::{repeat, repeat_n};

#[derive(Debug, Clone)]
struct Noisy(i64);

impl Drop for Noisy {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn noisy(base: i64) -> Vec<Noisy> {
    (0..4).map(|step| Noisy(base + step)).collect()
}

fn main() {
    let running: Vec<i64> = noisy(10)
        .into_iter()
        .scan(0, |total, item| {
            *total += item.0;
            if *total > 30 { None } else { Some(*total) }
        })
        .collect();
    println!("scan {running:?}");

    let mut pairs = [3, 1, 2].iter().scan(String::new(), |text, digit| {
        text.push_str(&digit.to_string());
        Some(text.len())
    });
    println!("scan state {:?} {:?}", pairs.next(), pairs.next());

    let scaled: Vec<Noisy> = noisy(20)
        .into_iter()
        .map_while(|item| (item.0 < 22).then(|| Noisy(item.0 * 10)))
        .collect();
    println!("map_while {scaled:?}");

    let rounds: Vec<Noisy> = noisy(30).into_iter().take(2).cycle().take(5).collect();
    println!("cycle {rounds:?}");
    let letters: String = "ab".chars().cycle().skip(1).take(5).collect();
    println!("cycle chars {letters}");
    println!("cycle empty {:?}", Vec::<u8>::new().iter().cycle().next());

    let copies: Vec<Noisy> = repeat(Noisy(40)).zip(0..2).map(|(item, _)| item).collect();
    println!("repeat {copies:?}");
    let counted: Vec<Noisy> = repeat_n(Noisy(50), 2).collect();
    println!("repeat_n {counted:?}");
    let numbered = repeat("ab")
        .zip(1..=3)
        .fold(String::new(), |mut out, (text, count)| {
            out.push_str(text);
            out.push_str(&count.to_string());
            out
        });
    println!("repeat str {numbered}");

    let numbers = [5, 3, 8, 1];
    let mut tens = numbers.iter().map(|value| value * 10);
    println!("len {} {:?} {}", tens.len(), tens.next(), tens.len());
    println!(
        "len adapters {} {} {}",
        numbers.iter().skip(1).len(),
        numbers.iter().enumerate().take(9).len(),
        (2..7).rev().len()
    );
    println!("end");
}
