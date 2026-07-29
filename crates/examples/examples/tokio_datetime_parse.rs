// The same RFC 3339 parsing on the parallel engine that `#[tokio::main]`
// selects. Both engines route through one shared core, so the numbers here
// must match the fast engine and the compiled build alike.

use chrono::{DateTime, Timelike};

#[tokio::main]
async fn main() {
    let utc = DateTime::parse_from_rfc3339("2026-07-29T21:20:00+00:00").unwrap();
    println!("utc timestamp: {}", utc.timestamp());
    println!("utc hour: {}", utc.hour());
    println!("utc rfc3339: {}", utc.to_rfc3339());

    let east = DateTime::parse_from_rfc3339("2026-07-29T23:20:00+02:00").unwrap();
    println!("east hour: {}", east.hour());
    println!("east rfc3339: {}", east.to_rfc3339());
    println!("same instant: {}", east.timestamp() == utc.timestamp());

    match DateTime::parse_from_rfc3339("nope") {
        Ok(dt) => println!("unexpectedly parsed: {}", dt.timestamp()),
        Err(e) => println!("error: {e}"),
    }
}
