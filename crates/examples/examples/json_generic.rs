#!/usr/bin/env rust

// The generic `fetch::<T>` resolves its type from the call site turbofish.

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

    let ids = parse::<Vec<String>>(r#"["x","y","z"]"#)?;
    println!("ids {} first {}", ids.len(), ids[0]);
    Ok(())
}
