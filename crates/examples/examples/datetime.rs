#!/usr/bin/env rust

// Current date and time with chrono. Only stable properties are printed.

use chrono::{Datelike, Utc};

fn main() {
    let now = Utc::now();
    println!("year is recent: {}", now.year() >= 2020);
    println!("month in range: {}", now.month() >= 1 && now.month() <= 12);

    let formatted = now.format("%Y-%m-%d").to_string();
    println!("formatted length: {}", formatted.len());
}
