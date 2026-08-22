use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

pub fn temp_script(src: &str) -> std::path::PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!("rustscript_test_{}_{}.rs", std::process::id(), id);
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, src).unwrap();
    path
}

/// A `Windows` separator reads as an escape in a string literal, and `Windows` accepts a forward
/// slash anyway.
pub fn embed_path(path: &std::path::Path) -> String {
    path.display().to_string().replace('\\', "/")
}

pub fn run(src: &str) -> String {
    let path = temp_script(src);
    let out = Command::new(env!("CARGO_BIN_EXE_rust"))
        .arg(&path)
        .env("RUSTSCRIPT_SKIP_CHECK", "1")
        .output()
        .expect("failed to launch rustscript");
    std::fs::remove_file(&path).unwrap();
    assert!(
        out.status.success(),
        "script failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn run_fail(src: &str) -> String {
    let path = temp_script(src);
    let out = Command::new(env!("CARGO_BIN_EXE_rust"))
        .arg(&path)
        .env("RUSTSCRIPT_SKIP_CHECK", "1")
        .output()
        .expect("failed to launch rustscript");
    std::fs::remove_file(&path).unwrap();
    assert!(!out.status.success(), "script unexpectedly succeeded");
    String::from_utf8_lossy(&out.stderr).into_owned()
}
