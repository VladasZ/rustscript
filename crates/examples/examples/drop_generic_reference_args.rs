#!/usr/bin/env rust

#[derive(Debug, Clone)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

// a reference goes into `X` by value, so the callee owns nothing and drops nothing
fn pick<X>(first: X, second: X, take_first: bool) -> X {
    if take_first { first } else { second }
}

fn make(id: i64) -> Vec<T> {
    vec![T(id)]
}

fn forward(left: &[T], right: &[T]) -> usize {
    let chosen = pick(left, right, false);
    println!("in forward");
    chosen.len()
}

fn main() {
    // temporaries behind the references drop when the statement ends, last made first
    let from_slices = pick(make(1).as_slice(), make(2).as_slice(), false).len();
    println!("a {from_slices}");
    let from_refs = pick(&make(3), &make(4), true).len();
    println!("b {from_refs}");

    let kept = make(5);
    let other = make(6);
    let both = pick(&kept, &other, false).len();
    println!("c {both}");
    println!("d {}", forward(&kept, &other));

    let one = T(7);
    let two = T(8);
    let inner = pick(&one, &two, true).0;
    println!("e {inner}");

    // a slice that comes back from the call is still a slice for the methods that take one
    let texts = vec![String::from("a"), String::from("b")];
    let tail = vec![String::from("b")];
    let empty: &[String] = &[];
    println!(
        "f {}",
        empty.starts_with(pick(texts.as_slice(), tail.as_slice(), true))
    );
    println!(
        "g {}",
        texts.ends_with(pick(texts.as_slice(), tail.as_slice(), false))
    );
    let mut grown = texts.clone();
    grown.extend_from_slice(pick(texts.as_slice(), tail.as_slice(), false));
    println!("h {grown:?} {texts:?} {tail:?}");
    println!("end");
}
