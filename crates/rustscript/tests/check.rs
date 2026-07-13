//! Tests for the `cargo check` validity gate. These invoke a real `cargo check`
//! on a synthesized project, which is slow, so they are ignored by default.
//! Run them with `cargo test --test check -- --ignored`.

use std::process::Command;

fn temp_script(src: &str, tag: &str) -> std::path::PathBuf {
    let name = format!("rustscript_check_{}_{}.rs", std::process::id(), tag);
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, src).unwrap();
    path
}

fn check(src: &str, tag: &str) -> std::process::Output {
    let path = temp_script(src, tag);
    let out = Command::new(env!("CARGO_BIN_EXE_rust"))
        .arg("check")
        .arg(&path)
        .output()
        .expect("failed to launch rustscript");
    std::fs::remove_file(&path).unwrap();
    out
}

#[test]
#[ignore = "runs real cargo check, slow"]
fn valid_script_passes_check() {
    let out = check(
        "fn main() { let x: i64 = 1; println!(\"{x}\"); }\n",
        "valid",
    );
    assert!(
        out.status.success(),
        "check failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
#[ignore = "runs real cargo check, slow"]
fn type_error_fails_check() {
    // Assigning a string to an i64 is a type error rustc must reject.
    let out = check(
        "fn main() { let x: i64 = \"nope\"; println!(\"{x}\"); }\n",
        "invalid",
    );
    assert!(!out.status.success(), "type error should fail the check");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not valid Rust"), "stderr was: {stderr}");
}
