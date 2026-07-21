//! Runs every example twice, once as a real compiled cargo example and once
//! through the rustscript interpreter, and asserts the stdout is byte for byte
//! identical. This is the strongest check that the interpreter matches the
//! behavior of the real Rust compiler.

use std::path::{Path, PathBuf};
use std::process::Command;

use pretty_assertions::assert_eq;

/// Examples that cannot be compared byte for byte. Network ones depend on a
/// live response, `args_echo` prints its own path as argv[0], which differs
/// between the compiled binary and the script, `registry_demo` is gated
/// behind a required-feature so cargo never builds a binary to compare against,
/// and `parallel` interleaves its task prints in a different order every run.
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

/// Directory that cargo drops compiled examples into, found relative to this
/// test binary so a custom target dir still works.
fn examples_bin_dir() -> PathBuf {
    let exe = std::env::current_exe().expect("current exe");
    // target/<profile>/deps/<testbin> -> target/<profile>/examples
    exe.parent().unwrap().parent().unwrap().join("examples")
}

fn scripts_dir() -> PathBuf {
    workspace_root().join("crates/examples/examples")
}

fn run(cmd: &mut Command) -> (bool, Vec<u8>) {
    let out = cmd.output().expect("failed to run command");
    (out.status.success(), out.stdout)
}

#[test]
fn interpreter_matches_compiler() {
    // Build every example as a real cargo binary first.
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
        // Creating a symlink on Windows needs privileges unix grants freely, so
        // run the comparison only where it is reliable. The build above still
        // covers that the example compiles cross-platform.
        if !cfg!(unix) && name == "symlink_demo" {
            continue;
        }

        let (compiled_ok, compiled_out) = run(&mut Command::new(bin_dir.join(&name)));
        let (script_ok, script_out) = run(Command::new(interp)
            .arg("run")
            .arg(&path)
            .env("RUSTSCRIPT_SKIP_CHECK", "1"));

        assert!(compiled_ok, "compiled example `{name}` exited with error");
        assert!(script_ok, "script `{name}` exited with error");
        assert_eq!(
            compiled_out,
            script_out,
            "output differs for `{name}`\n-- compiled --\n{}\n-- script --\n{}",
            String::from_utf8_lossy(&compiled_out),
            String::from_utf8_lossy(&script_out),
        );
    }
}
