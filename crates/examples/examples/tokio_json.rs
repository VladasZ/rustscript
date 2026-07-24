// Typed and dynamic serde_json on the parallel engine that `#[tokio::main]`
// selects: struct targets with optional and renamed fields, a generic helper
// resolving its turbofish, an annotated let coercion, and serialization back
// to text.

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
    #[serde(rename = "displayName")]
    display_name: Option<String>,
}

fn parse<T: for<'de> Deserialize<'de>>(text: &str) -> Result<T> {
    Ok(serde_json::from_str::<T>(text)?)
}

type Owners = Vec<Owner>;

#[tokio::main]
async fn main() -> Result<()> {
    let text = r#"{
        "host": "db1",
        "port": 5432,
        "tags": ["prod", "db"],
        "owner": { "name": "alice", "admin": true },
        "displayName": "primary"
    }"#;

    let server: Server = serde_json::from_str(text)?;
    println!("{} on port {}", server.host, server.port);
    println!("owner: {} admin={}", server.owner.name, server.owner.admin);
    match &server.display_name {
        Some(name) => println!("display: {name}"),
        None => println!("display: none"),
    }

    let again = serde_json::from_str::<Server>(text)?;
    println!("turbofish port: {}", again.port);

    let owners: Owners = serde_json::from_str(r#"[{"name":"bob","admin":false}]"#)?;
    println!("alias first {} admin={}", owners[0].name, owners[0].admin);

    let generic = parse::<Server>(text)?;
    println!("generic host: {}", generic.host);

    let round = serde_json::to_string(&owners)?;
    println!("round: {round}");
    Ok(())
}
