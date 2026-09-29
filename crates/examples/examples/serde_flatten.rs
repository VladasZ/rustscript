#!/usr/bin/env rust

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
struct Report {
    host: String,
    ok: bool,
}

#[derive(Serialize, Deserialize, Debug)]
struct LogLine {
    level: String,
    #[serde(flatten)]
    report: Report,
}

#[derive(Serialize, Deserialize, Debug)]
struct Tagged {
    id: u32,
    #[serde(flatten)]
    report: Report,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Serialize, Debug)]
struct Maybe {
    id: u32,
    #[serde(flatten)]
    report: Option<Report>,
}

fn main() -> anyhow::Result<()> {
    let line = LogLine {
        level: "info".to_string(),
        report: Report {
            host: "pc1".to_string(),
            ok: true,
        },
    };
    let text = serde_json::to_string(&line)?;
    println!("{text}");
    println!("{}", serde_json::to_string_pretty(&line)?);

    let back: LogLine = serde_json::from_str(&text)?;
    println!("{back:?}");

    let value = serde_json::to_value(&line)?;
    let from_value: LogLine = serde_json::from_value(value)?;
    println!("{from_value:?}");

    let tagged: Tagged =
        serde_json::from_str(r#"{"id":7,"host":"tc2","ok":false,"zone":"eu","n":3}"#)?;
    println!("{} {:?}", tagged.id, tagged.report);
    for (key, value) in &tagged.extra {
        println!("extra {key} = {value}");
    }
    println!("{}", serde_json::to_string(&tagged)?);

    let missing = serde_json::from_str::<LogLine>(r#"{"level":"warn","host":"x"}"#);
    println!("missing ok: {}", missing.is_err());

    let none = Maybe {
        id: 1,
        report: None,
    };
    println!("{}", serde_json::to_string(&none)?);
    Ok(())
}
