//! Runs every script in the top level `examples/` and asserts it exits cleanly. The `cargo check`
//! gate is skipped to stay fast.

use std::path::PathBuf;
use std::process::Command;

mod common;
mod parallel;

fn examples_dir() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../examples/examples")
        .canonicalize()
        .expect("examples dir")
}

#[test]
fn every_example_runs() {
    let mut cases: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(examples_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        // network examples need connectivity and `manual_` ones change real machine state
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("net_") || n.starts_with("manual_"))
        {
            continue;
        }
        // a symlink on `Windows` needs privileges
        let stem = path.file_stem().and_then(|n| n.to_str());
        if !cfg!(unix) && stem == Some("symlink_demo") {
            continue;
        }
        // registry, services and WMI exist only on `Windows`
        if !cfg!(windows) && matches!(stem, Some("registry_demo" | "service_demo" | "wmi_demo")) {
            continue;
        }
        cases.push(path);
    }
    cases.sort();

    let failed = parallel::failures(&cases, |path| {
        let label = path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("example");
        let (ok, _, err) = common::run(
            Command::new(env!("CARGO_BIN_EXE_rust"))
                .arg(path)
                .env("RUSTSCRIPT_SKIP_CHECK", "1"),
            label,
        );
        if ok {
            Ok(())
        } else {
            Err(format!(
                "example {} failed:\n{}",
                path.display(),
                String::from_utf8_lossy(&err)
            ))
        }
    });
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
    assert!(
        cases.len() >= 15,
        "expected many examples, ran {}",
        cases.len()
    );
}
