//! Runs every script in the top level `examples/` and asserts it exits
//! cleanly. The `cargo check` gate is skipped to stay fast.

use std::process::Command;

mod common;

fn examples_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../examples/examples")
        .canonicalize()
        .expect("examples dir")
}

#[test]
fn every_example_runs() {
    let mut ran = 0;
    for entry in std::fs::read_dir(examples_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        // Network examples need connectivity and `manual_` ones change real
        // machine state.
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("net_") || n.starts_with("manual_"))
        {
            continue;
        }
        // A symlink on `Windows` needs privileges.
        let stem = path.file_stem().and_then(|n| n.to_str());
        if !cfg!(unix) && stem == Some("symlink_demo") {
            continue;
        }
        // Registry, services and WMI exist only on `Windows`.
        if !cfg!(windows) && matches!(stem, Some("registry_demo" | "service_demo" | "wmi_demo")) {
            continue;
        }
        let label = stem.unwrap_or("example");
        let (ok, _, err) = common::run(
            Command::new(env!("CARGO_BIN_EXE_rust"))
                .arg(&path)
                .env("RUSTSCRIPT_SKIP_CHECK", "1"),
            label,
        );
        assert!(
            ok,
            "example {} failed:\n{}",
            path.display(),
            String::from_utf8_lossy(&err)
        );
        ran += 1;
    }
    assert!(ran >= 15, "expected many examples, ran {ran}");
}
