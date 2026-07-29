#!/usr/bin/env rust

// Parsing RFC 3339 timestamps with chrono. Every input is fixed text, so the
// output is stable and compiled and interpreted runs match byte for byte.

use chrono::{DateTime, Datelike, Timelike};

fn main() {
    let utc = DateTime::parse_from_rfc3339("2026-07-29T21:20:00.522580+00:00").unwrap();
    println!("utc timestamp: {}", utc.timestamp());
    println!("utc millis: {}", utc.timestamp_millis());
    println!("utc formatted: {}", utc.format("%Y-%m-%d %H:%M:%S"));
    println!("utc rfc3339: {}", utc.to_rfc3339());

    // The same instant written in a zone two hours east. The calendar fields
    // must read in that zone, not in UTC, which is what carrying the offset
    // through the parse buys.
    let east = DateTime::parse_from_rfc3339("2026-07-29T23:20:00+02:00").unwrap();
    println!("east timestamp: {}", east.timestamp());
    println!("east year: {}", east.year());
    println!("east day: {}", east.day());
    println!("east hour: {}", east.hour());
    println!("east minute: {}", east.minute());
    println!("east second: {}", east.second());
    println!("east rfc3339: {}", east.to_rfc3339());
    println!("same instant: {}", east.timestamp() == utc.timestamp());

    let west = DateTime::parse_from_rfc3339("2026-07-29T16:20:00-05:00").unwrap();
    println!("west hour: {}", west.hour());
    println!("west rfc3339: {}", west.to_rfc3339());

    match DateTime::parse_from_rfc3339("not a timestamp") {
        Ok(dt) => println!("unexpectedly parsed: {}", dt.timestamp()),
        Err(e) => println!("error: {e}"),
    }
}
