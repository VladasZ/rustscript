#!/usr/bin/env rust


use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

fn default_port() -> u16 {
    println!("default_port called");
    8080
}
fn default_name() -> String {
    "anon".to_string()
}

#[derive(Debug, Default, Deserialize, Serialize, PartialEq)]
struct Limits {
    max: u32,
    #[serde(default)]
    soft: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq)]
enum Mode {
    Fast,
    Slow,
}
#[allow(clippy::derivable_impls)]
impl Default for Mode {
    fn default() -> Self {
        Mode::Slow
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Cfg {
    app_name: String,
    #[serde(default)]
    ports: Vec<u16>,
    #[serde(default = "default_port")]
    main_port: u16,
    #[serde(default = "default_name", rename = "who")]
    owner: String,
    #[serde(default)]
    extra: HashMap<String, i64>,
    #[serde(default)]
    sorted: BTreeMap<String, i64>,
    #[serde(default)]
    limits: Limits,
    #[serde(default)]
    mode: Mode,
    #[serde(default = "Cfg::default_ratio")]
    ratio: f64,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    count: i32,
}
impl Cfg {
    fn default_ratio() -> f64 {
        0.5
    }
}

fn main() {
    let a: Cfg = serde_json::from_str(r#"{"appName":"x"}"#).unwrap();
    println!("{a:?}");
    let b: Cfg = serde_json::from_str(
        r#"{"appName":"y","mainPort":1,"who":"me","limits":{"max":3},"mode":"Fast","count":7}"#,
    )
    .unwrap();
    println!("{b:?}");
    let bad: Result<Cfg, _> = serde_json::from_str(r#"{"ports":[1]}"#);
    println!("{}", bad.is_err());
    let list: Vec<Cfg> =
        serde_json::from_str(r#"[{"appName":"p"},{"appName":"q","ports":[2]}]"#).unwrap();
    println!(
        "{:?}",
        list.iter()
            .map(|c| (c.main_port, c.ports.len()))
            .collect::<Vec<_>>()
    );
    let t: Cfg = toml::from_str("appName = \"t\"\n[limits]\nmax = 9\n").unwrap();
    println!("{t:?}");
    let y: Cfg = serde_yaml::from_str("appName: yy\nratio: 2.5\n").unwrap();
    println!("{y:?}");
    println!("{}", serde_json::to_string(&a).unwrap());
}
