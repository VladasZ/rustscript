#!/usr/bin/env rust

// A hand rolled percent encoder, the shape that exposed two interpreter bugs.
// Byte literal ranges in match patterns never matched, and a `&mut String`
// passed to a user function lost its mutations on return.

const HEX: &[u8; 16] = b"0123456789ABCDEF";

fn push_pct(out: &mut String, b: u8) {
    out.push('%');
    out.push(char::from(HEX[(b >> 4) as usize]));
    out.push(char::from(HEX[(b & 0x0f) as usize]));
}

fn quote_plus(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                out.push(char::from(b));
            }
            b' ' => out.push('+'),
            _ => push_pct(&mut out, b),
        }
    }
    out
}

fn classify(c: char) -> &'static str {
    match c {
        'a'..='z' => "lower",
        'A'..='Z' => "upper",
        '0'..='9' => "digit",
        _ => "other",
    }
}

fn bucket(n: i64) -> &'static str {
    match n {
        i64::MIN..=-1 => "negative",
        0 => "zero",
        1..10 => "small",
        10.. => "big",
    }
}

fn main() {
    println!("[{}]", quote_plus("Hello World_9.txt"));
    println!("[{}]", quote_plus("a/b:c?d=e&f"));
    println!("[{}]", quote_plus("тест"));

    println!(
        "{} {} {} {}",
        classify('q'),
        classify('Q'),
        classify('7'),
        classify('/')
    );
    println!("{} {} {} {}", bucket(-5), bucket(0), bucket(7), bucket(10));

    // A closure taking &mut must write back the same way a function does.
    let double = |s: &mut String| {
        let copy = s.clone();
        s.push_str(&copy);
    };
    let mut word = "ab".to_string();
    double(&mut word);
    println!("[{word}]");
}
