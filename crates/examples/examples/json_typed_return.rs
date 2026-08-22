#!/usr/bin/env rust

// The parse target comes from the signature only. Without it the renamed
// fields vanish.

use serde::Deserialize;

#[derive(Deserialize)]
struct Limit {
    kind: String,
    #[serde(rename = "usedPercent")]
    used: f64,
}

#[derive(Deserialize)]
struct Report {
    limits: Vec<Limit>,
}

const TEXT: &str = r#"{"limits": [
    {"kind": "session", "usedPercent": 3},
    {"kind": "weekly", "usedPercent": 64.5}
]}"#;

// A `map_err` on the tail keeps the return type's payload.
fn parse(text: &str) -> Result<Report, String> {
    serde_json::from_str(text).map_err(|e| format!("bad report, {e}"))
}

// An early `return`.
fn parse_early(text: &str) -> Result<Report, String> {
    if !text.is_empty() {
        return serde_json::from_str(text).map_err(|e| e.to_string());
    }
    Err("empty".to_string())
}

fn main() {
    for report in [parse(TEXT).unwrap(), parse_early(TEXT).unwrap()] {
        println!("limits: {}", report.limits.len());
        for limit in &report.limits {
            println!("  {} at {}%", limit.kind, limit.used);
        }
    }

    match parse("{").map(|r| r.limits.len()) {
        Ok(n) => println!("unexpectedly parsed {n}"),
        Err(why) => println!("error starts with: {}", &why[..12]),
    }
    println!("early empty: {:?}", parse_early("").err());
}
