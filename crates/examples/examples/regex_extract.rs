#!/usr/bin/env rust

use anyhow::Result;
use regex::Regex;

fn main() -> Result<()> {
    let text = "2026-07-04 ERROR disk, 2026-07-05 INFO ok, 2026-07-06 ERROR panic";

    let re = Regex::new(r"(\d{4})-(\d{2})-(\d{2}) (\w+)")?;

    println!("has a date: {}", re.is_match(text));

    let count = re.find_iter(text).count();
    println!("entries: {count}");

    // The iterator terminal `last` drains to the final item.
    let last_start = re.find_iter(text).last().map(|m| m.start()).unwrap_or(0);
    println!("last entry starts at: {last_start}");
    let last_level = re.captures_iter(text).last().map(|c| c[4].to_string()).unwrap_or_default();
    println!("last level: {last_level}");

    if let Some(caps) = re.captures(text) {
        println!("first year: {}", &caps[1]);
        println!("first level: {}", &caps[4]);
    }

    let named = Regex::new(r"(?P<year>\d{4})-(?P<month>\d{2})")?;
    if let Some(caps) = named.captures(text) {
        let year = caps.name("year").unwrap().as_str();
        let month = caps.name("month").unwrap().as_str();
        println!("named: {year}/{month}");
    }

    let errors = Regex::new(r"ERROR")?;
    let redacted = errors.replace_all(text, "WARN");
    println!("redacted: {redacted}");

    // regex::escape turns metacharacters into literals.
    println!("escaped: {}", regex::escape("a.b*c?"));
    Ok(())
}
