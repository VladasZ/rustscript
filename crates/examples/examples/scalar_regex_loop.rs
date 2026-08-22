#!/usr/bin/env rust

//! Regex `find_iter` loops the scalar for plan specializes, fallback edges
//! included.

use regex::Regex;

fn main() {
    let text = "w001 w202 w013 w204 w035 w206 w047 w208 w059 w200";
    let re = Regex::new(r"w0\d\d").unwrap();

    // The plan path.
    let mut found: i64 = 0;
    let mut spans: i64 = 0;
    let mut widths: i64 = 0;
    for m in re.find_iter(text) {
        found += 1;
        spans += i64::try_from(m.start()).unwrap() % 1000;
        widths += i64::try_from(m.end()).unwrap() - i64::try_from(m.start()).unwrap();
    }
    println!("found {found} spans {spans} widths {widths}");

    // The Result survives the loop, so writeback must build the real `Ok`.
    let mut last_fit = u32::try_from(0u64);
    let mut starts: i64 = 0;
    for m in re.find_iter(text) {
        last_fit = u32::try_from(m.start());
        starts += 1;
    }
    println!("starts {starts} last fit {}", last_fit.unwrap());

    // A break inside the plan.
    let mut early: i64 = 0;
    for m in re.find_iter(text) {
        early += i64::try_from(m.start()).unwrap();
        if early > 10 {
            break;
        }
    }
    println!("early {early}");

    // A string read stays generic.
    let mut text_len: i64 = 0;
    for m in re.find_iter(text) {
        text_len += i64::try_from(m.as_str().len()).unwrap();
    }
    println!("text len {text_len}");

    // A mid loop failover on an f32 read.
    let fraction = 2.5f32;
    let mut seen: i64 = 0;
    let mut caught: i64 = 0;
    for m in re.find_iter(text) {
        seen += 1;
        if m.start() > 20 && f64::from(fraction) > 2.0 {
            caught += 1;
        }
    }
    println!("seen {seen} caught {caught}");

    // An empty pattern must step the same.
    let empty = Regex::new("").unwrap();
    let mut gaps: i64 = 0;
    for m in empty.find_iter("abc") {
        gaps += i64::try_from(m.start()).unwrap() + i64::try_from(m.end()).unwrap();
    }
    println!("gaps {gaps}");
}
