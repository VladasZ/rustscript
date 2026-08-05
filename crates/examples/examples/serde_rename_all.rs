#!/usr/bin/env rust

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Person {
    display_name: String,
    account_id: i64,
    #[serde(rename = "emailAddress")]
    email: String,
    nick_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
struct Limits {
    max_retry_count: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Theme {
    accent_color: String,
}

fn main() {
    let p: Person = serde_json::from_str(
        r#"{"displayName":"James Wake","accountId":7,"emailAddress":"jw@example.com"}"#,
    )
    .unwrap();
    println!("{} {} {}", p.display_name, p.account_id, p.email);
    println!("{}", p.nick_name.is_none());
    println!("{}", serde_json::to_string(&p).unwrap());

    let l: Limits = serde_json::from_str(r#"{"MAX_RETRY_COUNT":5}"#).unwrap();
    println!("{}", l.max_retry_count);

    let t: Theme = serde_json::from_str(r#"{"accent-color":"teal"}"#).unwrap();
    println!("{}", t.accent_color);

    // A required field absent from the json must fail the parse, not bind a
    // hole that only explodes later.
    let missing = serde_json::from_str::<Person>(r#"{"displayName":"NoId"}"#);
    println!("{}", missing.is_err());
}
