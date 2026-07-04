#!/usr/bin/env rust

// Parse the same config from TOML and YAML into a typed struct.

use serde::Deserialize;

#[derive(Deserialize)]
struct Config {
    name: String,
    port: u16,
    enabled: bool,
}

fn main() -> anyhow::Result<()> {
    let toml_src = "name = \"widget\"\nport = 8080\nenabled = true";
    let from_toml: Config = toml::from_str(toml_src)?;
    println!("toml name: {}", from_toml.name);
    println!("toml port: {}", from_toml.port);

    let yaml_src = "name: widget\nport: 8080\nenabled: true\n";
    let from_yaml: Config = serde_yaml::from_str(yaml_src)?;
    println!("yaml name: {}", from_yaml.name);
    println!("yaml enabled: {}", from_yaml.enabled);
    Ok(())
}
