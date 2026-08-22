#!/usr/bin/env rust


use serde::Deserialize;
use serde_yaml::from_str as from_yaml_str;
use toml::from_str;

#[derive(Deserialize)]
struct Config {
    name: String,
    port: u16,
    enabled: bool,
}

fn main() -> anyhow::Result<()> {
    let toml_src = "name = \"widget\"\nport = 8080\nenabled = true";
    let from_toml: Config = from_str(toml_src)?;
    println!("toml name: {}", from_toml.name);
    println!("toml port: {}", from_toml.port);

    let yaml_src = "name: widget\nport: 8080\nenabled: true\n";
    let from_yaml: Config = from_yaml_str(yaml_src)?;
    println!("yaml name: {}", from_yaml.name);
    println!("yaml enabled: {}", from_yaml.enabled);
    Ok(())
}
