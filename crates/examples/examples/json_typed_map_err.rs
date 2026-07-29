#!/usr/bin/env rust

// A typed json parse whose chain rewrites the error before the `?`. The
// annotation still names the struct to build, so the renamed keys must be
// honored. Getting this wrong is silent, the parse succeeds and every renamed
// field comes back empty, which is why it is worth an example of its own.

use serde::Deserialize;

#[derive(Deserialize)]
struct Account {
    #[serde(rename = "emailAddress")]
    email: String,
    #[serde(rename = "rateLimitTier")]
    tier: Option<String>,
}

#[derive(Deserialize)]
struct Config {
    #[serde(rename = "oauthAccount")]
    account: Option<Account>,
    #[serde(rename = "numStartups")]
    startups: i64,
}

const TEXT: &str = r#"{
    "oauthAccount": {"emailAddress": "someone@example.com", "rateLimitTier": "max_20x"},
    "numStartups": 41,
    "ignored": {"nested": [1, 2, 3]}
}"#;

fn read(text: &str) -> Result<Config, String> {
    let config: Config = serde_json::from_str(text).map_err(|e| format!("bad config, {e}"))?;
    Ok(config)
}

fn main() {
    let config = read(TEXT).unwrap();
    println!("startups: {}", config.startups);
    match &config.account {
        Some(account) => {
            println!("email: {}", account.email);
            println!("tier: {:?}", account.tier);
        }
        None => println!("no account"),
    }

    match read("{ not json }") {
        Ok(_) => println!("unexpectedly parsed"),
        Err(why) => println!("error starts with: {}", &why[..12]),
    }
}
