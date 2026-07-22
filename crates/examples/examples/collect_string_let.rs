#!/usr/bin/env rust

// `collect` is type driven, and the interpreter can only see the target in a
// turbofish or in the annotation of the surrounding `let`. The annotated let
// shape comes from a real script that split a line into head and rest, where
// the chars collected into a char list instead of a String and the next call
// on the result failed.

fn split_whitespace_once(s: &str) -> Vec<String> {
    let trimmed = s.trim_start();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let chars: Vec<char> = trimmed.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n && !chars[i].is_whitespace() {
        i += 1;
    }
    if i == n {
        return vec![trimmed.to_string()];
    }
    let head: String = chars[0..i].iter().collect();
    while i < n && chars[i].is_whitespace() {
        i += 1;
    }
    let rest: String = chars[i..n].iter().collect();
    vec![head, rest]
}

fn main() {
    let parts = split_whitespace_once("Token:   abc def ");
    println!("head [{}]", parts[0]);
    println!("rest [{}] trimmed [{}]", parts[1], parts[1].trim());

    let taken: String = "abcdefgh".chars().take(3).collect();
    println!("taken [{taken}]");

    let turbo = "xyz".chars().collect::<String>();
    println!("turbo [{turbo}]");

    // An annotated let inside the closure must not eat the outer hint.
    let words = ["alpha".to_string(), "beta".to_string()];
    let initials: String = words
        .iter()
        .map(|w| {
            let first: String = w.chars().take(1).collect();
            first
        })
        .collect();
    println!("initials [{initials}] len {}", initials.len());

    // A plain collect into a vec keeps working next to the string ones.
    let back: Vec<char> = taken.chars().collect();
    println!("back {} {}", back.len(), back[0]);
}
