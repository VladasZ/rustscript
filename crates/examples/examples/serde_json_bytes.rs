#!/usr/bin/env rust

//! `serde_json::to_vec` and `from_slice`, the byte twins of `to_string` and `from_str`. A
//! snapshot is saved as bytes and read back into its struct.

use std::collections::BTreeMap;
use std::fs;

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize, Serialize)]
struct Snapshot {
    hashes: BTreeMap<String, String>,
    skipped: usize,
    #[serde(default)]
    note: Option<String>,
}

fn save(path: &std::path::Path, snapshot: &Snapshot) -> anyhow::Result<()> {
    fs::write(path, serde_json::to_vec(snapshot)?)?;
    Ok(())
}

fn load(path: &std::path::Path) -> anyhow::Result<Snapshot> {
    let snapshot = serde_json::from_slice(&fs::read(path)?)?;
    Ok(snapshot)
}

fn main() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("snapshot.json");
    let mut hashes = BTreeMap::new();
    hashes.insert("b.txt".to_string(), "22".to_string());
    hashes.insert("a.txt".to_string(), "11".to_string());
    save(
        &path,
        &Snapshot {
            hashes,
            skipped: 3,
            note: None,
        },
    )?;
    println!("{}", fs::read_to_string(&path)?);
    println!("{:?}", load(&path)?);

    let bytes = serde_json::to_vec(&vec![1, 2, 3])?;
    println!("{bytes:?}");
    let pretty = serde_json::to_vec_pretty(&Snapshot::default())?;
    println!("{}", String::from_utf8(pretty)?);

    let numbers: Vec<u32> = serde_json::from_slice(b"[4, 5, 6]")?;
    println!("{numbers:?}");
    let value: serde_json::Value = serde_json::from_slice(br#"{"x": [true, null]}"#)?;
    println!("{value}");

    let typed = serde_json::from_slice::<Snapshot>(br#"{"skipped": 1}"#);
    println!("missing: {}", typed.unwrap_err());
    let broken = serde_json::from_slice::<serde_json::Value>(b"{\"k\": \"\xff\"}");
    println!("not utf-8: {}", broken.is_err());
    Ok(())
}
