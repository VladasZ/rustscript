//! Runtime error traces. A failing script must name the failing line and the
//! script call chain, on both engines, with deep recursion capped.

use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_script(src: &str) -> std::path::PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!("rustscript_trace_{}_{}.rs", std::process::id(), id);
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, src).unwrap();
    path
}

/// Run a script that is expected to fail and return its stderr.
fn run_fail(src: &str) -> String {
    let path = temp_script(src);
    let out = Command::new(env!("CARGO_BIN_EXE_rust"))
        .arg("run")
        .arg(&path)
        .env("RUSTSCRIPT_SKIP_CHECK", "1")
        .output()
        .expect("failed to launch rustscript");
    std::fs::remove_file(&path).unwrap();
    assert!(!out.status.success(), "script unexpectedly succeeded");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn runtime_error_reports_line_and_backtrace() {
    let err = run_fail(
        r#"
fn helper(v: &Vec<i64>) -> i64 {
    v[10]
}

fn main() {
    let nums = vec![1, 2, 3];
    println!("{}", helper(&nums));
}
"#,
    );
    assert!(
        err.contains("index out of bounds: the len is 3 but the index is 10"),
        "stderr was: {err}"
    );
    assert!(err.contains("panicked at"), "stderr was: {err}");
    assert!(
        err.contains("at helper (rustscript_trace"),
        "stderr was: {err}"
    );
    assert!(err.contains(".rs:3)"), "stderr was: {err}");
    assert!(
        err.contains("at main (rustscript_trace"),
        "stderr was: {err}"
    );
    assert!(err.contains(".rs:8)"), "stderr was: {err}");
}

#[test]
fn tokio_error_reports_line_and_backtrace() {
    let err = run_fail(
        r#"
async fn fetch(v: Vec<i64>) -> i64 {
    v[99]
}

#[tokio::main]
async fn main() {
    println!("{}", fetch(vec![1]).await);
}
"#,
    );
    assert!(
        err.contains("index out of bounds: the len is 1 but the index is 99"),
        "stderr was: {err}"
    );
    assert!(
        err.contains("at fetch (rustscript_trace"),
        "stderr was: {err}"
    );
    assert!(err.contains(".rs:3)"), "stderr was: {err}");
}

#[test]
fn unknown_method_error_names_the_struct() {
    let err = run_fail(
        r#"
struct Dog;

impl Dog {
    fn speak(&self) -> String { "woof".to_string() }
}

fn main() {
    let d = Dog;
    println!("{}", d.speak());
    d.fly();
}
"#,
    );
    assert!(
        err.contains("unknown method `fly` on struct `Dog`"),
        "stderr was: {err}"
    );
}

#[test]
fn stack_overflow_trace_is_capped() {
    let err = run_fail(
        r#"
fn down(n: i64) -> i64 {
    down(n + 1)
}

fn main() {
    println!("{}", down(0));
}
"#,
    );
    assert!(err.contains("stack overflow"), "stderr was: {err}");
    assert!(err.contains("more frames"), "stderr was: {err}");
    let lines = err.lines().count();
    assert!(
        lines < 25,
        "trace should be capped, got {lines} lines:\n{err}"
    );
}

#[test]
fn closure_error_traces_through_the_closure() {
    let err = run_fail(
        r#"
fn main() {
    let vals = vec![1, 2, 3];
    let bad = |i: i64| -> i64 { vals[i as usize + 10] };
    println!("{}", bad(0));
}
"#,
    );
    assert!(err.contains("out of bounds"), "stderr was: {err}");
    assert!(
        err.contains("at <closure> (rustscript_trace"),
        "stderr was: {err}"
    );
    assert!(err.contains(".rs:4)"), "stderr was: {err}");
}
