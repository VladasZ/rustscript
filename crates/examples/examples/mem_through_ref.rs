#!/usr/bin/env rust

// `take`, `replace` and `swap` get a `&mut T` binding here, not a `&mut place` written at the
// call.

use std::mem::{replace, swap, take};

#[derive(Debug, Default)]
struct Tally {
    hits: u32,
    name: String,
}

struct Buf {
    word: String,
    words: Vec<String>,
}

fn flush(words: &mut Vec<String>, word: &mut String) {
    if !word.is_empty() {
        words.push(take(word));
    }
}

fn rotate(n: &mut i64, other: &mut i64) -> i64 {
    swap(n, other);
    replace(n, 100)
}

fn reset(tally: &mut Tally) -> Tally {
    take(tally)
}

fn main() {
    let mut words = Vec::new();
    let mut word = String::from("one");
    flush(&mut words, &mut word);
    word.push_str("two");
    flush(&mut words, &mut word);
    flush(&mut words, &mut word);
    println!("words {words:?}, left {word:?}");

    let (mut a, mut b) = (1, 2);
    let old = rotate(&mut a, &mut b);
    println!("replace gave {old}, a {a}, b {b}");

    let mut tally = Tally {
        hits: 3,
        name: "t".into(),
    };
    let was = reset(&mut tally);
    println!(
        "was {} {:?}, now {} {:?}",
        was.hits, was.name, tally.hits, tally.name
    );

    let mut buf = Buf {
        word: "x".into(),
        words: Vec::new(),
    };
    let held = &mut buf;
    let slot = &mut held.word;
    let got = take(slot);
    held.words.push(got);
    let mut other = vec![String::from("y")];
    swap(&mut held.words, &mut other);
    println!("buf {:?} {:?}, other {other:?}", buf.word, buf.words);
}
