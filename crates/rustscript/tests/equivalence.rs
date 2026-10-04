//! Runs every example compiled and interpreted and asserts identical stdout. This is the
//! strongest check the interpreter has.

use std::collections::HashMap;
use std::env::consts::EXE_SUFFIX;
use std::fs::read_dir;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
mod parallel;

/// Network ones depend on a live response, `args_echo` prints its own path, `registry_demo` is behind
/// a required feature, and `parallel` prints in a different order every run.
const SKIP: &[&str] = &[
    "net_get",
    "net_query",
    "args_echo",
    "registry_demo",
    "service_demo",
    "wmi_demo",
    "manual_service_write",
    "parallel",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

/// Found relative to this test binary so a custom target dir still works.
fn examples_bin_dir() -> PathBuf {
    let exe = std::env::current_exe().expect("current exe");
    // target/<profile>/deps/<testbin> -> target/<profile>/examples
    exe.parent().unwrap().parent().unwrap().join("examples")
}

fn scripts_dir() -> PathBuf {
    workspace_root().join("crates/examples/examples")
}

/// The file cargo built for each example, `fib-7ef051b6ca9f7f55`, by example name. The plain
/// `fib` next to it is a fresh copy cargo makes on every build whose feature set differs from
/// the one before, and `cargo test --workspace` and the build below differ. macOS checks a
/// binary on its first launch, half a second each and one at a time, so launching the copies
/// cost about 5 minutes on every run. The built file keeps its place until its source
/// changes. It is the one with the size and the time of the copy.
fn stable_binaries(bin_dir: &Path) -> HashMap<String, PathBuf> {
    let mut out = HashMap::new();
    let Ok(entries) = read_dir(bin_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(stem) = stem.strip_suffix(EXE_SUFFIX) else {
            continue;
        };
        let Some((name, hash)) = stem.rsplit_once('-') else {
            continue;
        };
        if hash.len() != 16 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let copy = bin_dir.join(format!("{name}{EXE_SUFFIX}"));
        let (Ok(built), Ok(copied)) = (path.metadata(), copy.metadata()) else {
            continue;
        };
        if built.len() == copied.len() && built.modified().ok() == copied.modified().ok() {
            out.insert(name.to_string(), path);
        }
    }
    out
}

#[test]
fn interpreter_matches_compiler() {
    let build = Command::new(env!("CARGO"))
        .args(["build", "--examples", "-p", "rustscript-examples"])
        .current_dir(workspace_root())
        .status()
        .expect("failed to build examples");
    assert!(build.success(), "cargo build --examples failed");

    let bin_dir = examples_bin_dir();
    let scripts = scripts_dir();
    let interp = env!("CARGO_BIN_EXE_rust");

    let mut cases: Vec<(String, PathBuf)> = Vec::new();
    for entry in read_dir(&scripts).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let name = path.file_stem().unwrap().to_str().unwrap().to_string();
        if SKIP.contains(&name.as_str()) {
            continue;
        }
        // A symlink on `Windows` needs privileges, so compare only where it is reliable. The
        // build above still proves the example compiles.
        if !cfg!(unix) && name == "symlink_demo" {
            continue;
        }
        cases.push((name, path));
    }
    cases.sort();

    let binaries = stable_binaries(&bin_dir);
    let failed = parallel::failures(&cases, |(name, path)| {
        let binary = binaries
            .get(name)
            .cloned()
            .unwrap_or_else(|| bin_dir.join(name));
        let (compiled_ok, compiled_out, compiled_err) =
            common::run(&mut Command::new(binary), &format!("compiled {name}"));
        let (script_ok, script_out, script_err) = common::run(
            Command::new(interp)
                .arg(path)
                .env("RUSTSCRIPT_SKIP_CHECK", "1"),
            &format!("script {name}"),
        );
        if !compiled_ok {
            return Err(format!(
                "compiled example `{name}` exited with error:\n{}",
                String::from_utf8_lossy(&compiled_err)
            ));
        }
        if !script_ok {
            return Err(format!(
                "script `{name}` exited with error:\n{}",
                String::from_utf8_lossy(&script_err)
            ));
        }
        if compiled_out != script_out {
            return Err(format!(
                "output differs for `{name}`\n-- compiled --\n{}\n-- script --\n{}",
                String::from_utf8_lossy(&compiled_out),
                String::from_utf8_lossy(&script_out),
            ));
        }
        Ok(())
    });
    assert!(
        failed.is_empty(),
        "{} of {} examples disagree:\n\n{}",
        failed.len(),
        cases.len(),
        failed.join("\n\n")
    );
}
