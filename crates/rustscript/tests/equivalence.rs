//! Runs every example compiled and interpreted and asserts identical stdout. This is the
//! strongest check the interpreter has.

use std::path::{Path, PathBuf};
use std::process::Command;

use pretty_assertions::assert_eq;

mod common;

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

    for entry in std::fs::read_dir(&scripts).unwrap() {
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

        let (compiled_ok, compiled_out, compiled_err) = common::run(
            &mut Command::new(bin_dir.join(&name)),
            &format!("compiled {name}"),
        );
        let (script_ok, script_out, script_err) = common::run(
            Command::new(interp)
                .arg(&path)
                .env("RUSTSCRIPT_SKIP_CHECK", "1"),
            &format!("script {name}"),
        );

        assert!(
            compiled_ok,
            "compiled example `{name}` exited with error:\n{}",
            String::from_utf8_lossy(&compiled_err)
        );
        assert!(
            script_ok,
            "script `{name}` exited with error:\n{}",
            String::from_utf8_lossy(&script_err)
        );
        assert_eq!(
            compiled_out,
            script_out,
            "output differs for `{name}`\n-- compiled --\n{}\n-- script --\n{}",
            String::from_utf8_lossy(&compiled_out),
            String::from_utf8_lossy(&script_out),
        );
    }
}
