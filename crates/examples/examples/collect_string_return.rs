#!/usr/bin/env rust

// `collect` is type driven, and a function's own `-> String` is the third place
// that names the target, after a turbofish and an annotated `let`. The shape
// comes from a real script whose timestamp helper returned a bare
// `chars().take(19).collect()`, which built a char list and stamped
// run-['2', '0', '2', '6', ..].log into a filename instead of failing.

// The plain case: the body's trailing expression is the collect.
fn stamp(iso: &str) -> String {
    iso.replace([':', '.'], "-")
        .replace('T', "_")
        .chars()
        .take(19)
        .collect()
}

// A tail `if` returns from both branches, so both collects are the return value.
fn initials(name: &str, short: bool) -> String {
    if short {
        name.chars().take(1).collect()
    } else {
        name.chars().take(4).collect()
    }
}

// A tail `match` does the same across its arms.
fn head(s: &str, how: u8) -> String {
    match how {
        0 => s.chars().take(0).collect(),
        1 => s.chars().take(1).collect(),
        _ => s.chars().rev().collect(),
    }
}

// An early `return` hands back a collect too, and the tail is a plain String.
fn label(s: &str) -> String {
    if s.is_empty() {
        return "abc".chars().collect();
    }
    s.to_string()
}

// A `let else` commonly returns, and that return is not part of the let's value.
fn first_word(s: &str) -> String {
    let Some(idx) = s.find(' ') else {
        return s.chars().collect();
    };
    s[0..idx].to_string()
}

// The return type must not reach a closure's own collect, which builds a Vec
// here and would be broken by inheriting the outer String.
fn lengths(words: &[String]) -> String {
    let counts: Vec<usize> = words
        .iter()
        .map(|w| {
            let chars: Vec<char> = w.chars().collect();
            chars.len()
        })
        .collect();
    let mut out = String::new();
    for c in &counts {
        out.push_str(&c.to_string());
    }
    out
}

// A function that does not return String must keep collecting into a Vec, even
// when the tail is a bare collect.
fn chars_of(s: &str) -> Vec<char> {
    s.chars().collect()
}

fn main() {
    let s = stamp("2026-07-29T07:31:19.123Z");
    println!("stamp [{s}] len {}", s.len());

    println!(
        "initials [{}] [{}]",
        initials("alphabet", true),
        initials("alphabet", false)
    );

    println!(
        "head [{}] [{}] [{}]",
        head("abc", 0),
        head("abc", 1),
        head("abc", 9)
    );

    println!("label [{}] [{}]", label(""), label("given"));

    println!(
        "first_word [{}] [{}]",
        first_word("alpha beta"),
        first_word("single")
    );

    let words = ["alpha".to_string(), "be".to_string()];
    println!("lengths [{}]", lengths(&words));

    let cs = chars_of("abc");
    println!("chars_of {} {}", cs.len(), cs[0]);
}
