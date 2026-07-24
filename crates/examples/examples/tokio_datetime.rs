// Chrono datetimes on the parallel engine that `#[tokio::main]` selects,
// backed by the same shared core as the fast engine. Only stable properties
// are printed, so compiled and interpreted output stay identical.

use chrono::{Datelike, Utc};

#[tokio::main]
async fn main() {
    let now = Utc::now();
    println!("year is recent: {}", now.year() >= 2020);
    println!("month in range: {}", now.month() >= 1 && now.month() <= 12);
    println!("day in range: {}", now.day() >= 1 && now.day() <= 31);
    println!("timestamp positive: {}", now.timestamp() > 0);
    println!(
        "millis consistent: {}",
        now.timestamp_millis() / 1000 == now.timestamp()
    );

    let formatted = now.format("%Y-%m-%d").to_string();
    println!("formatted length: {}", formatted.len());
    println!("rfc3339 has T: {}", now.to_rfc3339().contains('T'));
}
