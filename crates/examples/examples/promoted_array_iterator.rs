#!/usr/bin/env rust

//! An array of literals is promoted to a static, so an iterator a `let` keeps over it still
//! finds the items after the statement, also in a script that has a `Drop` type.

struct Noisy(i64);

impl Drop for Noisy {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn main() {
    let guard = Noisy(1);
    let mut plus = [3, 1, 2].iter().map(|value| value + 1);
    println!("{:?} {:?}", plus.next(), plus.next());
    let mut pairs = [(1, 'a'), (2, 'b')].iter().rev();
    println!("{:?} {:?}", pairs.next(), pairs.next());
    let mut zeros = [0u8; 3].iter().enumerate().skip(1);
    println!("{:?} {:?}", zeros.next(), zeros.next());
    let words = ["x", "yy"].iter().map(|word| word.len());
    println!("{:?}", words.collect::<Vec<_>>());
    let slice: &[i32] = &[-4, 5];
    println!("{slice:?} {}", guard.0);
}
