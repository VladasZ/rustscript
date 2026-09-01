use pretty_assertions::assert_eq;

use super::common::{run, run_fail, temp_script};

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

/// The gate runs before `main`, so the call on a cold branch fails the script with nothing printed
/// and no panic.
#[test]
fn a_path_call_the_interpreter_lacks_stops_the_script_before_it_runs() {
    let path = temp_script(
        r#"
fn main() {
    println!("side effect");
    if std::env::args().count() > 100 {
        let d = chrono::Duration::hours(1);
        println!("{d}");
    }
}
"#,
    );
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_rust"))
        .arg(&path)
        .env("RUSTSCRIPT_SKIP_CHECK", "1")
        .output()
        .expect("failed to launch rustscript");
    std::fs::remove_file(&path).unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "the gate must fail the script");
    assert!(
        err.contains("`chrono::Duration::hours` is not implemented by the interpreter"),
        "stderr was: {err}"
    );
    assert!(!err.contains("panicked"), "stderr was: {err}");
    assert!(out.stdout.is_empty(), "nothing may run before the gate");
}

/// Recursion through a closure nests the VM on the host stack. The cap ends it with the script
/// panic, never with the process abort of a real stack overflow.
#[test]
fn recursion_through_a_closure_hits_the_depth_cap_cleanly() {
    let path = temp_script(
        r#"
fn walk(n: u64) -> u64 {
    if n == 0 { 0 } else { (0..1u64).map(|_| walk(n - 1)).sum::<u64>() + 1 }
}
fn main() {
    println!("{}", walk(150_000));
}
"#,
    );
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_rust"))
        .arg(&path)
        .env("RUSTSCRIPT_SKIP_CHECK", "1")
        .output()
        .expect("failed to launch rustscript");
    std::fs::remove_file(&path).unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(101), "stderr was: {err}");
    assert!(err.contains("stack overflow:"), "stderr was: {err}");
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
