//! Runs every script in `tests/cases` twice, once compiled by rustc and once
//! through the rustscript interpreter, and asserts the stdout is byte for
//! byte identical. These cases exist to pin down numeric semantics, `as`
//! casts, float formatting, and float comparison, so their sources use the
//! constructs on purpose. They are compiled here at test time rather than as
//! cargo targets, the same way the differential harness compiles its
//! generated programs.

use std::path::{Path, PathBuf};
use std::process::Command;

use pretty_assertions::assert_eq;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn run(cmd: &mut Command) -> (bool, Vec<u8>) {
    let out = cmd.output().expect("failed to run command");
    (out.status.success(), out.stdout)
}

#[test]
fn semantics_cases_match_compiler() {
    let cases = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases");
    let out_dir = std::env::temp_dir().join(format!("rustscript-semantics-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).expect("create output dir");
    let interp = env!("CARGO_BIN_EXE_rust");

    for entry in std::fs::read_dir(&cases).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let name = path.file_stem().unwrap().to_str().unwrap().to_string();
        let binary = out_dir.join(&name);

        // Overflow checks match the debug profile the equivalence examples
        // build under, so aborting arithmetic aborts in both runs.
        let compile = Command::new("rustc")
            .args(["--edition", "2024", "-C", "overflow-checks=yes", "-o"])
            .arg(&binary)
            .arg(&path)
            .current_dir(workspace_root())
            .output()
            .expect("failed to run rustc");
        assert!(
            compile.status.success(),
            "rustc rejected `{name}`:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );

        let (compiled_ok, compiled_out) = run(&mut Command::new(&binary));
        let (script_ok, script_out) = run(Command::new(interp)
            .arg("run")
            .arg(&path)
            .env("RUSTSCRIPT_SKIP_CHECK", "1"));

        assert!(compiled_ok, "compiled case `{name}` exited with error");
        assert!(script_ok, "script case `{name}` exited with error");
        assert_eq!(
            compiled_out,
            script_out,
            "output differs for `{name}`\n-- compiled --\n{}\n-- script --\n{}",
            String::from_utf8_lossy(&compiled_out),
            String::from_utf8_lossy(&script_out),
        );
    }

    std::fs::remove_dir_all(&out_dir).ok();
}
