#!/usr/bin/env rust

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
struct Owner {
    name: String,
    admin: bool,
}

#[derive(Serialize, Deserialize, Debug)]
struct Server {
    host: String,
    port: i64,
    tags: Vec<String>,
    owner: Owner,
}

fn main() -> Result<()> {
    let text = r#"{
        "host": "db1",
        "port": 5432,
        "tags": ["prod", "db"],
        "owner": { "name": "alice", "admin": true }
    }"#;

    let server: Server = serde_json::from_str(text)?;
    println!("{} on port {}", server.host, server.port);
    println!("first tag: {}", server.tags[0]);
    println!("owner: {} admin={}", server.owner.name, server.owner.admin);

    let again = serde_json::from_str::<Server>(text)?;
    println!("turbofish port: {}", again.port);

    let list: Vec<Owner> = serde_json::from_str(r#"[{"name":"bob","admin":false}]"#)?;
    println!("list len {} first {}", list.len(), list[0].name);
    Ok(())
}
