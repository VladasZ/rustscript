//! The calls in the examples whose result type the inference pass does not know, against the
//! list in `untyped_calls.txt`. An untyped call runs on what the value says, which is where a
//! lost integer width or a wrong `Default` comes from. A new entry fails the test, so the list
//! only shrinks. Type the call in `compile/infer`, or run with `RUSTSCRIPT_BLESS=1` to accept it.

use std::collections::BTreeSet;
use std::env::var_os;
use std::fs::{read_dir, read_to_string, write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

mod common;
mod parallel;

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/untyped_calls.txt")
}

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/examples")
}

/// Every `receiver name` pair the census reports over the examples.
fn found() -> BTreeSet<String> {
    let mut scripts: Vec<PathBuf> = read_dir(examples_dir())
        .expect("examples dir")
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("rs"))
        .collect();
    scripts.sort();
    let out: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
    let failed = parallel::failures(&scripts, |path| {
        let (ok, stdout, stderr) = common::run(
            Command::new(env!("CARGO_BIN_EXE_rust"))
                .arg("types")
                .arg(path),
            &format!("types {}", path.display()),
        );
        if !ok {
            return Err(format!(
                "`rust types {}` failed:\n{}",
                path.display(),
                String::from_utf8_lossy(&stderr)
            ));
        }
        let text = String::from_utf8_lossy(&stdout);
        let mut calls = out.lock().expect("calls lock");
        for line in text.lines() {
            // `file:line receiver name`
            if let Some((_, call)) = line.split_once(' ') {
                calls.insert(call.to_string());
            }
        }
        Ok(())
    });
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
    out.into_inner().expect("calls lock")
}

#[test]
fn no_new_untyped_calls() {
    let found = found();
    if var_os("RUSTSCRIPT_BLESS").is_some() {
        let lines: Vec<&str> = found.iter().map(String::as_str).collect();
        let text = lines.join("\n") + "\n";
        write(baseline_path(), text).expect("write baseline");
        return;
    }
    let baseline = read_to_string(baseline_path()).expect("read baseline");
    let known: BTreeSet<&str> = baseline.lines().collect();
    let new: Vec<&String> = found
        .iter()
        .filter(|call| !known.contains(call.as_str()))
        .collect();
    assert!(
        new.is_empty(),
        "{} call(s) with no known result type, add a row in `compile/infer` for each:\n{new:#?}",
        new.len()
    );
}
