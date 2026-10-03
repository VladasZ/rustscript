#!/usr/bin/env rust

//! An index or a count the std hands out is a `usize`, so `!` flips 64 unsigned bits and the
//! integer methods answer.

use regex::Regex;

fn main() {
    let items: Vec<u32> = vec![7, 8, 9];
    for (i, item) in items.iter().enumerate() {
        println!("{} {} {item}", !i, i.is_power_of_two());
    }
    for (i, item) in items.iter().enumerate().rev() {
        println!("{} {item}", i.leading_zeros());
    }
    let pairs: Vec<(usize, u32)> = items.clone().into_iter().enumerate().collect();
    println!("{}", !pairs[2].0);

    let mut last: usize = 0;
    for (i, _item) in items.iter().enumerate() {
        let before: usize = std::mem::replace(&mut last, i);
        println!("{before}");
    }
    println!("{}", !last);

    let at = items.iter().position(|x| *x == 8).unwrap_or_default();
    println!("{} {}", !at, at.count_ones());
    let from_end = items.iter().rposition(|x| *x > 7).unwrap_or_default();
    println!("{} {}", !from_end, from_end.is_power_of_two());

    let text = "a-b-c";
    let first = text.find('-').unwrap_or_default();
    let final_dash = text.rfind('-').unwrap_or_default();
    println!("{} {}", !first, !final_dash);
    for (offset, ch) in text.char_indices() {
        println!("{} {ch}", offset.trailing_zeros());
    }
    println!("{}", !text.chars().count());

    let word = Regex::new(r"(\w+)-(\w+)").expect("a valid pattern");
    if let Some(found) = word.find("xx ab-cd") {
        println!("{} {}", !found.start(), found.end().is_power_of_two());
    }
    if let Some(groups) = word.captures("ab-cd") {
        println!("{}", !groups.len());
    }
}
