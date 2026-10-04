#!/usr/bin/env rust

//! `clone_from` and `clone_from_slice` follow std. A `Vec` keeps its storage, drops the items
//! past the new length, clones into the ones it keeps and appends the rest. An `Option` that
//! is `Some` on both sides clones into its payload. Anything else clones first and then drops
//! the old value.

#[derive(Debug, Clone, PartialEq)]
struct Noisy(i64);
impl Drop for Noisy {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
fn main() {
    let mut first = vec![1];
    let second = vec![2, 3];
    first.clone_from(&second);
    first.push(4);
    println!("{first:?} {second:?}");
    println!("-- vec");
    let mut items = vec![Noisy(1), Noisy(2), Noisy(3)];
    let short = vec![Noisy(4)];
    items.clone_from(&short);
    println!("{items:?}");
    println!("-- grow");
    items.clone_from(&vec![Noisy(5), Noisy(6)]);
    println!("{items:?}");
    println!("-- nested slice");
    let mut rows = vec![vec![Noisy(11), Noisy(12), Noisy(13)], Vec::new()];
    let fresh = vec![vec![Noisy(14)], vec![Noisy(15)]];
    rows[..].clone_from_slice(&fresh);
    println!("{rows:?}");
    println!("-- option");
    let mut maybe = Some(vec![Noisy(21), Noisy(22)]);
    maybe.clone_from(&Some(vec![Noisy(23)]));
    println!("{maybe:?}");
    maybe.clone_from(&None);
    println!("{maybe:?}");
    println!("-- struct");
    let mut pair = (Noisy(31), String::from("a"));
    pair.clone_from(&(Noisy(32), String::from("b")));
    let mut text = String::from("x");
    text.clone_from(&pair.1);
    println!("{pair:?} {text}");
    println!("end");
}
