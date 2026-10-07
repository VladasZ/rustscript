#!/usr/bin/env rust

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
struct Package {
    name: String,
    manifest_path: PathBuf,
    readme: Option<PathBuf>,
    #[serde(default)]
    targets: Vec<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize)]
enum Source {
    Local(PathBuf),
    Registry(String),
}

#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    target_directory: PathBuf,
}

fn main() {
    let text = r#"{
        "packages": [
            {
                "name": "demo",
                "manifest_path": "/a/b/Cargo.toml",
                "readme": "/a/b/README.md",
                "targets": ["/a/b/src/main.rs", "/a/b/src/lib.rs"]
            },
            { "name": "bare", "manifest_path": "/c/Cargo.toml", "readme": null }
        ],
        "target_directory": "/a/target"
    }"#;
    let metadata: Metadata = serde_json::from_str(text).unwrap();
    for package in &metadata.packages {
        println!("{}", package.name);
        println!("  parent {:?}", package.manifest_path.parent());
        println!("  display {}", package.manifest_path.display());
        println!("  name {:?}", package.manifest_path.file_name());
        let folder = package.manifest_path.parent().unwrap();
        println!("  join {}", folder.join("src").display());
        println!(
            "  readme {:?}",
            package.readme.as_ref().and_then(|p| p.extension())
        );
        for target in &package.targets {
            println!("  target {:?}", target.file_stem());
        }
        println!("  debug {package:?}");
    }
    let debug = metadata.target_directory.join("debug");
    println!("{}", debug.display());

    let back = serde_json::to_string(&metadata.packages[0]).unwrap();
    println!("{back}");

    let value = serde_json::json!({ "name": "v", "manifest_path": "/v/Cargo.toml" });
    let from_value: Package = serde_json::from_value(value).unwrap();
    println!("{:?}", from_value.manifest_path.parent());

    let source: Source = serde_json::from_str(r#"{"Local":"/src/crate"}"#).unwrap();
    match &source {
        Source::Local(path) => println!("local {:?}", path.parent()),
        Source::Registry(name) => println!("registry {name}"),
    }
    println!("{}", serde_json::to_string(&source).unwrap());

    let alone: PathBuf = serde_json::from_str(r#""/x/y.txt""#).unwrap();
    println!("{:?}", alone.extension());

    let wrong = serde_json::from_str::<Package>(r#"{"name":"n","manifest_path":5}"#);
    println!("{}", wrong.unwrap_err());

    let from_toml: Package =
        toml::from_str("name = \"t\"\nmanifest_path = \"/t/Cargo.toml\"\n").unwrap();
    println!("{:?}", from_toml.manifest_path.parent());
}
