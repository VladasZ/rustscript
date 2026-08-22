#!/usr/bin/env rust

//! A script trait on builtin types is keyed by the written type, so `Vec<u8>`
//! and `Vec<String>` get their own bodies.

trait Describe {
    fn describe(&self) -> String;

    fn shout(&self) -> String {
        self.describe().to_uppercase()
    }
}

impl Describe for u8 {
    fn describe(&self) -> String {
        format!("u8={self}")
    }
}

impl Describe for i32 {
    fn describe(&self) -> String {
        format!("i32={}", self.abs())
    }
}

impl Describe for f64 {
    fn describe(&self) -> String {
        format!("f64={self:.2}")
    }
}

impl Describe for bool {
    fn describe(&self) -> String {
        if *self {
            "yes".to_string()
        } else {
            "no".to_string()
        }
    }
}

impl Describe for char {
    fn describe(&self) -> String {
        format!("char={self:?}")
    }
}

impl Describe for String {
    fn describe(&self) -> String {
        format!("text of {} bytes", self.len())
    }
}

impl Describe for Vec<u8> {
    fn describe(&self) -> String {
        format!(
            "bytes summing to {}",
            self.iter().map(|b| u32::from(*b)).sum::<u32>()
        )
    }
}

impl Describe for Vec<String> {
    fn describe(&self) -> String {
        self.join("+")
    }
}

/// Through a generic bound.
fn both<T: Describe>(left: &T, right: &T) -> String {
    format!("{} / {}", left.describe(), right.describe())
}

fn main() {
    let byte: u8 = 200;
    let negative: i32 = -7;
    let ratio: f64 = 2.0 / 3.0;
    let flag = std::env::args().count() > 1000;
    let letter = 'q';
    let text = String::from("héllo");
    let bytes: Vec<u8> = vec![1, 2, 250];
    let words = vec![String::from("a"), String::from("b")];
    let empty: Vec<u8> = Vec::new();

    println!("{}", byte.describe());
    println!("{}", negative.describe());
    println!("{}", ratio.describe());
    println!("{}", flag.describe());
    println!("{}", letter.describe());
    println!("{}", text.describe());
    println!("{}", bytes.describe());
    println!("{}", words.describe());
    println!("{}", empty.describe());
    let nested: Vec<Vec<u8>> = Vec::new();
    println!("{}", nested.concat().describe());
    println!("{}", words.shout());
    println!("{}", (i32::from(byte) * -2).describe());

    println!("{}", both(&byte, &1u8));
    println!("{}", both(&words, &vec![String::from("z")]));
}
