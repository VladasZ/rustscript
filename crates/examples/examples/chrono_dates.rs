#!/usr/bin/env rust

//! The chrono surface the teams scripts use. Naive parsing, `and_hms_opt`, `and_utc`,
//! `with_timezone`, weekdays, signed `Duration` deltas, and `DateTime` comparison. All on fixed
//! dates, so compiled and interpreted output match byte for byte.

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, Utc};

fn main() {
    let naive = NaiveDateTime::parse_from_str("2026-09-02T10:00:00", "%Y-%m-%dT%H:%M:%S").unwrap();
    let day = NaiveDate::parse_from_str("2026-09-02", "%Y-%m-%d").unwrap();
    let midnight = day.and_hms_opt(0, 0, 0).unwrap().and_utc();
    println!("{} {}", naive.and_utc().timestamp(), midnight.timestamp());
    println!("{}", day.and_hms_opt(25, 0, 0).is_none());
    println!("{}", NaiveDate::parse_from_str("nope", "%Y-%m-%d").is_err());

    let parsed = DateTime::parse_from_rfc3339("2026-09-02T10:00:00+03:00").unwrap();
    let utc = parsed.with_timezone(&Utc);
    // the local view is the same instant, so the timestamp is identical in any zone
    let local = utc.with_timezone(&Local);
    println!("{} {}", utc.timestamp(), local.timestamp());
    println!(
        "{} {}",
        utc.weekday().num_days_from_sunday(),
        utc.weekday().num_days_from_monday()
    );

    let mut shifted = utc;
    shifted += Duration::nanoseconds(1_500_000_000);
    let yesterday = utc - Duration::milliseconds(86_400_000);
    println!(
        "{} {}",
        shifted.timestamp() - utc.timestamp(),
        utc.timestamp() - yesterday.timestamp()
    );
    let restored = DateTime::from_timestamp(utc.timestamp(), 0).unwrap();
    println!("{} {} {}", yesterday < utc, utc == restored, shifted >= utc);
    println!(
        "{}",
        (Duration::hours(2) + Duration::minutes(30)).num_minutes()
    );
    println!(
        "{} {}",
        restored.timestamp(),
        DateTime::from_timestamp(i64::MAX, 0).is_none()
    );
}
