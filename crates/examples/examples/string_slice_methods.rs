#!/usr/bin/env rust

// `s[range].method()` once failed with "cannot index String" because
// `SetIndex` had no string arm. The writeback now splices the slice back
// into the base, through cells, fields and other projections.

struct Label {
    text: String,
}

fn main() {
    // `replace` is on the mutating list for `Option::replace`, but string
    // `replace` does not mutate.
    let word = String::from("abcdef");
    println!("replace [{}]", word[2..].replace('c', "x"));
    println!("base [{word}]");

    // The shape `setup.rs` broke on.
    let home = String::from("/Users/someone");
    let shell = String::from("/Users/someone/dev/thing/shell");
    println!("rel [{}]", shell[home.len()..].replace('\\', "/"));

    // A real mutation must reach the base.
    let mut mixed = String::from("hello world");
    mixed[6..].make_ascii_uppercase();
    println!("upper [{mixed}]");
    mixed[..5].make_ascii_uppercase();
    println!("both [{mixed}]");

    let mut incl = String::from("abcdef");
    incl[1..=3].make_ascii_uppercase();
    println!("inclusive [{incl}]");

    // The base lives in a cell.
    let mut captured = String::from("closure case");
    let mut shout = || captured[..7].make_ascii_uppercase();
    shout();
    println!("cell [{captured}]");

    let mut label = Label {
        text: String::from("field case"),
    };
    label.text[..5].make_ascii_uppercase();
    println!("field [{}]", label.text);

    // 2 chained projections. The annotation keeps this a vector, clippy
    // would ask for an array.
    let mut words: Vec<String> = vec![String::from("one"), String::from("two")];
    words[1][..2].make_ascii_uppercase();
    println!("vec [{}] [{}]", words[0], words[1]);

    let mut param = String::from("param case");
    upper_head(&mut param);
    println!("param [{param}]");
}

fn upper_head(text: &mut str) {
    text[..5].make_ascii_uppercase();
}
