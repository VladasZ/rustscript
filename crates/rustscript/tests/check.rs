//! Tests for the `cargo check` gate. They run a real `cargo check`, which is slow, so they are ignored
//! by default. Run with `cargo test --test check -- --ignored`.

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
    // a type error `rustc` must reject
    let out = check(
        "fn main() { let x: i64 = \"nope\"; println!(\"{x}\"); }\n",
        "invalid",
    );
    assert!(!out.status.success(), "type error should fail the check");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not valid Rust"), "stderr was: {stderr}");
}

#[test]
#[ignore = "runs real cargo check, slow"]
fn diagnostics_name_the_real_script_file() {
    // the script is mirrored into the cache project under its own name, so the arrow line must
    // point at that name and not `main.rs`
    let out = check(
        "fn main() { let x: i64 = \"nope\"; println!(\"{x}\"); }\n",
        "diagname",
    );
    assert!(!out.status.success(), "type error should fail the check");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let expected = format!("rustscript_check_{}_diagname.rs", std::process::id());
    assert!(
        stderr.contains(&expected),
        "diagnostic should name {expected}, stderr was: {stderr}"
    );
    assert!(
        !stderr.contains("main.rs"),
        "diagnostic should not name main.rs, stderr was: {stderr}"
    );
}

fn build(src: &str, tag: &str, args: &[&str]) -> (std::path::PathBuf, std::process::Output) {
    let path = temp_script(src, tag);
    let out = Command::new(env!("CARGO_BIN_EXE_rust"))
        .arg("build")
        .arg(&path)
        .args(args)
        .output()
        .expect("failed to launch rustscript");
    (path, out)
}

#[test]
#[ignore = "runs real cargo build, slow"]
fn build_compiles_runs_and_then_hits_the_cache() {
    let src = "fn main() {\n    let arg = std::env::args().nth(1).unwrap_or_default();\n    println!(\"built {arg}\");\n}\n";
    let (path, first) = build(src, "build", &["one"]);
    let stderr = String::from_utf8_lossy(&first.stderr);
    assert!(first.status.success(), "build failed:\n{stderr}");
    assert_eq!(String::from_utf8_lossy(&first.stdout), "built one\n");
    assert!(
        stderr.contains("rust: compiling"),
        "first run must compile, stderr was: {stderr}"
    );

    // the same source again is a cache hit, so nothing compiles
    let second = Command::new(env!("CARGO_BIN_EXE_rust"))
        .args(["build", path.to_str().unwrap(), "two"])
        .output()
        .expect("failed to launch rustscript");
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(second.status.success(), "cached build failed:\n{stderr}");
    assert_eq!(String::from_utf8_lossy(&second.stdout), "built two\n");
    assert!(
        !stderr.contains("rust: compiling"),
        "second run must hit the cache, stderr was: {stderr}"
    );

    // `cmp` as the first script argument is the same path
    let cmp = Command::new(env!("CARGO_BIN_EXE_rust"))
        .args([path.to_str().unwrap(), "cmp", "three"])
        .output()
        .expect("failed to launch rustscript");
    assert!(
        cmp.status.success(),
        "cmp run failed:\n{}",
        String::from_utf8_lossy(&cmp.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&cmp.stdout), "built three\n");
    std::fs::remove_file(&path).unwrap();
}
