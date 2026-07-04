#!/usr/bin/env rust

use anyhow::Result;
use serde::Serialize;

#[derive(Serialize)]
struct Config {
    name: String,
    retries: i64,
    verbose: bool,
}

fn main() -> Result<()> {
    let cfg = Config {
        name: "nightly-job".to_string(),
        retries: 3,
        verbose: true,
    };
    let text = serde_json::to_string_pretty(&cfg)?;
    println!("{text}");

    let parsed: serde_json::Value = serde_json::from_str(&text)?;
    let name = parsed["name"].as_str().unwrap();
    let retries = parsed["retries"].as_i64().unwrap();
    println!("parsed name={name} retries={retries}");
    Ok(())
}
