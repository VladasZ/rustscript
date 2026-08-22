#!/usr/bin/env rust

// A function's `-> String` names the `collect` target. A bare `chars().take(19).collect()` in tail
// position must be a String, not a char list. A real script stamped `run-['2', '0', '2', '6',
// ..].log` into a filename because of this.

// the tail expression
fn stamp(iso: &str) -> String {
    iso.replace([':', '.'], "-")
        .replace('T', "_")
        .chars()
        .take(19)
        .collect()
}

// both branches of a tail `if`
fn initials(name: &str, short: bool) -> String {
    if short {
        name.chars().take(1).collect()
    } else {
        name.chars().take(4).collect()
    }
}

// the arms of a tail `match`
fn head(s: &str, how: u8) -> String {
    match how {
        0 => s.chars().take(0).collect(),
        1 => s.chars().take(1).collect(),
        _ => s.chars().rev().collect(),
    }
}

// an early `return`
fn label(s: &str) -> String {
    if s.is_empty() {
        return "abc".chars().collect();
    }
    s.to_string()
}

// a `let else` return is not part of the let's value
fn first_word(s: &str) -> String {
    let Some(idx) = s.find(' ') else {
        return s.chars().collect();
    };
    s[0..idx].to_string()
}

// the return type must not reach a closure's own collect
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

// a non String return type still collects into a Vec
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
