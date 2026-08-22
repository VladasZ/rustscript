use pretty_assertions::assert_eq;

use super::common::{run, run_fail};

#[test]
fn shebang_is_ignored() {
    let out = run("#!/usr/bin/env rust\nfn main() { println!(\"ok\"); }\n");
    assert_eq!(out, "ok\n");
}

#[test]
fn error_from_main_exits_nonzero() {
    let err = run_fail(
        r#"
fn main() -> anyhow::Result<()> {
    anyhow::bail!("boom");
}
"#,
    );
    assert!(err.contains("boom"), "stderr was: {err}");
}

#[test]
fn panic_exits_nonzero() {
    let err = run_fail(
        r#"
fn main() {
    let v: Vec<i64> = vec![];
    println!("{}", v[0]);
}
"#,
    );
    assert!(!err.is_empty());
}

#[test]
#[ignore = "runs real cargo check, slow"]
fn check_reports_a_method_the_interpreter_lacks() {
    // valid Rust, but the interpreter has no `rposition`, so the coverage gate must catch it
    // before running
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("script.rs");
    std::fs::write(
        &file,
        r#"
#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let v = vec![1, 2, 3];
    println!("{:?}", v.iter().rposition(|x| *x == 2));
}
"#,
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_rust"))
        .args(["check", file.to_str().unwrap()])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "check should fail, stderr: {err}");
    assert!(err.contains("rposition"), "stderr was: {err}");
}

#[test]
#[ignore = "runs real cargo check, slow"]
fn check_stays_quiet_on_a_script_the_interpreter_supports() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("script.rs");
    std::fs::write(
        &file,
        r#"
fn main() {
    let v = vec![1, 2, 3];
    let doubled: Vec<i64> = v.iter().map(|x| x * 2).collect();
    println!("{} {}", doubled.len(), "ab".repeat(2));
}
"#,
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_rust"))
        .args(["check", file.to_str().unwrap()])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "check should pass, stderr: {err}");
}
