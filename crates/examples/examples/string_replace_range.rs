#!/usr/bin/env rust

// `String::replace_range` edits the receiver in place, through a local, a `&mut String` param and
// a struct field. A clone taken before the edit keeps its own contents.

struct Line {
    text: String,
}

fn stamp(s: &mut String, with: &str) {
    s.replace_range(..3, with);
}

fn main() {
    let mut s = String::from("hello world");
    let snapshot = s.clone();
    s.replace_range(0..5, "HELLO");
    println!("{s}");
    println!("snapshot {snapshot}");

    // open and inclusive ranges, a longer and a shorter replacement
    s.replace_range(6.., "rust script");
    println!("{s}");
    s.replace_range(..=4, "hi");
    println!("{s}");
    s.replace_range(2..2, " there");
    println!("{s}");
    s.replace_range(.., "");
    println!("empty {:?} len {}", s, s.len());

    // byte offsets around a multibyte char
    let mut word = String::from("naïve");
    word.replace_range(2..4, "i");
    println!("{word}");

    let mut id = String::from("abc-123");
    stamp(&mut id, "XYZ");
    println!("{id}");

    let mut line = Line {
        text: String::from("[ ] todo"),
    };
    line.text.replace_range(1..2, "x");
    println!("{}", line.text);
}
