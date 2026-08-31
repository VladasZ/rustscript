#!/usr/bin/env rust

use anyhow::Result;
use regex::{Regex, escape};

fn main() -> Result<()> {
    let text = "2026-07-04 ERROR disk, 2026-07-05 INFO ok, 2026-07-06 ERROR panic";

    let re = Regex::new(r"(\d{4})-(\d{2})-(\d{2}) (\w+)")?;

    println!("has a date: {}", re.is_match(text));

    let count = re.find_iter(text).count();
    println!("entries: {count}");

    let last_start = re.find_iter(text).last().map_or(0, |m| m.start());
    println!("last entry starts at: {last_start}");
    let last_level = re
        .captures_iter(text)
        .last()
        .map(|c| c[4].to_string())
        .unwrap_or_default();
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
    println!("first only: {}", errors.replace(text, "WARN"));
    println!("first two: {}", errors.replacen(text, 2, "WARN"));
    // a zero limit means no limit, the same as `replace_all`
    println!("no limit: {}", errors.replacen(text, 0, "WARN"));
    println!("swapped: {}", re.replacen(text, 1, "$3.$2.$1 $4"));

    // the replace family hands back a `Cow<str>`, so the borrowed view and the owned one
    // both have to read the same text
    let patched = errors.replacen(text, 1, "WARN");
    let view: &str = patched.as_ref();
    println!("as_ref: {view}");
    println!("into_owned: {}", errors.replace(text, "WARN").into_owned());

    println!("escaped: {}", escape("a.b*c?"));
    Ok(())
}
