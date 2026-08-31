#!/usr/bin/env rust

// A header map iterates in an order the crate does not promise, so every listing here is sorted
// before it is printed.

use reqwest::header::{HeaderMap, HeaderValue};

fn main() {
    let mut map = HeaderMap::new();
    map.append("set-cookie", HeaderValue::from_static("a=1"));
    map.append("set-cookie", HeaderValue::from_static("b=2"));
    map.insert("content-type", HeaderValue::from_static("text/plain"));

    let mut cookies: Vec<String> = Vec::new();
    for value in map.get_all("set-cookie") {
        cookies.push(value.to_str().unwrap_or("").to_string());
    }
    println!("cookies: {cookies:?}");

    println!("len: {}", map.len());
    println!("keys_len: {}", map.keys_len());
    println!("is empty: {}", map.is_empty());
    println!("has content type: {}", map.contains_key("content-type"));
    println!("has accept: {}", map.contains_key("accept"));

    match map.get("content-type") {
        Some(value) => println!("content type: {}", value.to_str().unwrap_or("")),
        None => println!("no content type"),
    }

    let replaced = map.insert("content-type", HeaderValue::from_static("application/json"));
    let old = replaced.map(|value| value.to_str().unwrap_or("").to_string());
    println!("replaced: {old:?}");
    println!("len after insert: {}", map.len());

    let seen = map.append("accept", HeaderValue::from_static("*/*"));
    println!("accept was already there: {seen}");

    let mut lines: Vec<String> = map
        .iter()
        .map(|(name, value)| format!("{}={}", name.as_str(), value.to_str().unwrap_or("")))
        .collect();
    lines.sort();
    println!("all: {lines:?}");

    let mut names: Vec<String> = map.keys().map(|name| name.as_str().to_string()).collect();
    names.sort();
    println!("names: {names:?}");

    let mut values: Vec<String> = map
        .values()
        .map(|value| value.to_str().unwrap_or("").to_string())
        .collect();
    values.sort();
    println!("values: {values:?}");

    let copy = map.clone();
    let removed = map.remove("set-cookie");
    let gone = removed.map(|value| value.to_str().unwrap_or("").to_string());
    println!("removed: {gone:?}");
    println!("len after remove: {}", map.len());
    println!("the copy still has them: {}", copy.len());

    match HeaderValue::from_str("built at run time") {
        Ok(value) => println!("parsed: {}", value.to_str().unwrap_or("")),
        Err(e) => println!("parse failed: {e}"),
    }
    println!(
        "a newline is rejected: {}",
        HeaderValue::from_str("bad\nvalue").is_err()
    );
}
