#!/usr/bin/env rust

use anyhow::Result;

fn main() -> Result<()> {
    let resp = reqwest::blocking::get("https://example.com")?;
    let status = resp.status().as_u16();
    println!("status: {status}");

    let body: String = reqwest::blocking::get("https://example.com")?.text()?;
    println!("fetched {} bytes", body.len());
    println!("looks like html: {}", body.contains("<html"));
    Ok(())
}
