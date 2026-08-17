#!/usr/bin/env rust

// Method calls on a string slice, `s[range].method()`. Names on the builtin
// mutating list compile the receiver as a place, and the writeback used to
// fail with "cannot index String" because SetIndex had no string arm. The
// writeback now splices the slice bytes back into the base string, and the
// place chain carries the splice home when the base lives in a cell, a
// field, or another projection.

struct Label {
    text: String,
}

fn main() {
    // `replace` is on the mutating list for `Option::replace`. String
    // `replace` does not mutate, so the base must stay unchanged.
    let word = String::from("abcdef");
    println!("replace [{}]", word[2..].replace('c', "x"));
    println!("base [{word}]");

    // A slice start that is a method call, the shape setup.rs broke on.
    let home = String::from("/Users/someone");
    let shell = String::from("/Users/someone/dev/thing/shell");
    println!("rel [{}]", shell[home.len()..].replace('\\', "/"));

    // A method that really mutates through the slice must reach the base.
    let mut mixed = String::from("hello world");
    mixed[6..].make_ascii_uppercase();
    println!("upper [{mixed}]");
    mixed[..5].make_ascii_uppercase();
    println!("both [{mixed}]");

    // An inclusive range.
    let mut incl = String::from("abcdef");
    incl[1..=3].make_ascii_uppercase();
    println!("inclusive [{incl}]");

    // A captured string, the base lives in a cell and the splice must land
    // there through the place chain.
    let mut captured = String::from("closure case");
    let mut shout = || captured[..7].make_ascii_uppercase();
    shout();
    println!("cell [{captured}]");

    // A string behind a field.
    let mut label = Label {
        text: String::from("field case"),
    };
    label.text[..5].make_ascii_uppercase();
    println!("field [{}]", label.text);

    // A string behind an element, two chained projections. The annotation
    // keeps this a real vector rather than the array clippy would ask for.
    let mut words: Vec<String> = vec![String::from("one"), String::from("two")];
    words[1][..2].make_ascii_uppercase();
    println!("vec [{}] [{}]", words[0], words[1]);

    // A string behind a `&mut` parameter.
    let mut param = String::from("param case");
    upper_head(&mut param);
    println!("param [{param}]");
}

fn upper_head(text: &mut str) {
    text[..5].make_ascii_uppercase();
}
