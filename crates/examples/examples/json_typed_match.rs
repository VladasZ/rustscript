#!/usr/bin/env rust

// A `match` on the parse must not lose the annotated struct. The annotation on the `let` names the
// payload, so it has to reach the `from_str` in the scrutinee, the same way it reaches one under a
// `?`. Getting it wrong is silent, every field comes back empty.

use serde::Deserialize;

#[derive(Deserialize)]
struct Leaf {
    #[serde(rename = "endCursor")]
    cursor: Option<String>,
    value: i64,
}

#[derive(Deserialize)]
struct Mid {
    #[serde(rename = "pullRequest")]
    leaf: Option<Leaf>,
}

#[derive(Deserialize)]
struct Top {
    data: Option<Mid>,
}

const TEXT: &str = r#"{"data": {"pullRequest": {"endCursor": "abc123", "value": 41}}}"#;

fn main() {
    let arm: Top = match serde_json::from_str(TEXT) {
        Ok(parsed) => parsed,
        Err(e) => {
            println!("failed: {e}");
            return;
        }
    };
    let leaf = arm.data.and_then(|mid| mid.leaf);
    match &leaf {
        Some(l) => println!("arm value: {}, cursor: {:?}", l.value, l.cursor),
        None => println!("arm missed the leaf"),
    }

    // The block bodied arm carries the annotation too.
    let block: Top = match serde_json::from_str(TEXT) {
        Ok(parsed) => {
            println!("parsed in a block");
            parsed
        }
        Err(_) => return,
    };
    println!(
        "block value: {:?}",
        block.data.and_then(|m| m.leaf).map(|l| l.value)
    );

    // An arm that does not hand the payload out keeps its own type.
    let counted: i64 = match serde_json::from_str::<Top>(TEXT) {
        Ok(_) => 7,
        Err(_) => -1,
    };
    println!("counted: {counted}");

    let broken: Result<Top, _> = serde_json::from_str("{ not json }");
    match broken {
        Ok(_) => println!("unexpectedly parsed"),
        Err(e) => println!("error is reported: {}", !e.to_string().is_empty()),
    }
}
