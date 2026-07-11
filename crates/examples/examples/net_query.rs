#!/usr/bin/env rust

// HTTP with query parameters and a cookie-persisting client. Needs network, so
// it is not part of the automated run.

use std::time::Duration;

fn main() -> anyhow::Result<()> {
    let client = reqwest::blocking::Client::builder()
        .cookie_store(true)
        .timeout(Duration::from_secs(10))
        .build()?;

    // The client keeps cookies between calls.
    let first = client
        .get("https://httpbin.org/cookies/set")
        .query(&[("session", "abc123")])
        .send()?;
    println!("set status: {}", first.status().as_u16());
    first.text()?;

    let second = client.get("https://httpbin.org/cookies").send()?;
    let body = second.text()?;
    println!("cookie echoed back: {}", body.contains("abc123"));
    Ok(())
}
