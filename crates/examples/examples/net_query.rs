#!/usr/bin/env rust

// HTTP with query parameters and a cookie-persisting agent. Needs network, so
// it is not part of the automated run.

use std::time::Duration;

fn main() -> anyhow::Result<()> {
    let agent = ureq::agent();

    // The agent keeps cookies between calls.
    let mut first = agent
        .get("https://httpbin.org/cookies/set")
        .query("session", "abc123")
        .config()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .call()?;
    println!("set status: {}", first.status());
    first.body_mut().read_to_string()?;

    let mut second = agent.get("https://httpbin.org/cookies").call()?;
    let body = second.body_mut().read_to_string()?;
    println!("cookie echoed back: {}", body.contains("abc123"));
    Ok(())
}
