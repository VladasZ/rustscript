#!/usr/bin/env rust

// Typed json through a generic helper, with `#[serde(rename)]` and optional
// fields. The generic `fetch::<T>` resolves its concrete type from the
// turbofish at the call site, the same way a real deserialize helper reads.

use anyhow::Result;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
struct Page {
    data: Vec<Row>,
}

#[derive(Deserialize, Debug)]
struct Row {
    id: String,
    #[serde(rename = "bundleId")]
    bundle_id: String,
    version: Option<String>,
}

fn parse<T: for<'de> Deserialize<'de>>(text: &str) -> Result<T> {
    Ok(serde_json::from_str::<T>(text)?)
}

fn main() -> Result<()> {
    let text = r#"{"data":[
        {"id":"1","bundleId":"com.a","version":"3"},
        {"id":"2","bundleId":"com.b"}
    ]}"#;

    let page = parse::<Page>(text)?;
    for row in &page.data {
        let version = match &row.version {
            Some(v) => v.clone(),
            None => "none".to_string(),
        };
        println!("{} {} v={version}", row.id, row.bundle_id);
    }

    // The same helper at a different concrete type.
    let ids = parse::<Vec<String>>(r#"["x","y","z"]"#)?;
    println!("ids {} first {}", ids.len(), ids[0]);
    Ok(())
}
