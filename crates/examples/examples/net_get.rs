#!/usr/bin/env rustscript

use anyhow::Result;

fn main() -> Result<()> {
    let resp = ureq::get("https://example.com").call()?;
    let status = resp.status().as_u16();
    println!("status: {status}");

    let body: String = ureq::get("https://example.com")
        .call()?
        .body_mut()
        .read_to_string()?;
    println!("fetched {} bytes", body.len());
    println!("looks like html: {}", body.contains("<html"));
    Ok(())
}
