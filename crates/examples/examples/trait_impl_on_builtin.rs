#!/usr/bin/env rust

// A script trait on a builtin type adds to that type, it does not replace it. The bridge methods
// of `char`, `i64` and `String` must still be there.

trait Describe {
    fn describe(&self) -> String;
}

impl Describe for char {
    fn describe(&self) -> String {
        format!("char {self:?}")
    }
}

impl Describe for i64 {
    fn describe(&self) -> String {
        format!("i64 {self}")
    }
}

impl Describe for String {
    fn describe(&self) -> String {
        format!("string {self:?}")
    }
}

fn same_letter(left: char, right: char) -> bool {
    left.eq_ignore_ascii_case(&right)
}

fn magnitude(value: i64) -> i64 {
    value.abs()
}

fn shout(text: String) -> String {
    text.to_uppercase()
}

fn main() {
    println!("{}", 'x'.describe());
    println!("{}", same_letter('A', 'a'));
    println!("{}", 'A'.to_ascii_lowercase());
    println!("{}", 'A'.is_ascii_uppercase());

    println!("{}", 7i64.describe());
    println!("{}", magnitude(-9));
    println!("{}", 7i64.pow(2));

    println!("{}", String::from("hi").describe());
    println!("{}", shout(String::from("hi")));
    println!("{}", String::from("hi").len());
}
