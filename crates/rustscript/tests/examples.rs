//! Runs every script in the top level `examples/` directory and asserts it
//! exits cleanly. The `cargo check` gate is skipped so this stays fast, a
//! separate suite covers the gate itself.

use std::process::Command;

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
        // Network examples need connectivity, so they are not run here.
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("net_"))
        {
            continue;
        }
        let out = Command::new(env!("CARGO_BIN_EXE_rustscript"))
            .arg("run")
            .arg(&path)
            .env("RUSTSCRIPT_SKIP_CHECK", "1")
            .output()
            .expect("failed to launch rustscript");
        assert!(
            out.status.success(),
            "example {} failed:\n{}",
            path.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        ran += 1;
    }
    assert!(ran >= 15, "expected many examples, ran {ran}");
}
