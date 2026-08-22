//! A script inside a cargo crate can use a local `path` dependency. The interpreter grafts it in
//! from source and the checker adds it as a real path dependency.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use pretty_assertions::assert_eq;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// `shared` has no external deps so the check is fast.
fn fixture() -> (PathBuf, PathBuf) {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("rustscript_crate_{}_{}", std::process::id(), id));
    if root.exists() {
        std::fs::remove_dir_all(&root).unwrap();
    }

    write(
        &root.join("shared/Cargo.toml"),
        r#"[package]
name = "shared"
version = "0.0.0"
edition = "2024"
[dependencies]
[workspace]
"#,
    );
    write(
        &root.join("shared/src/lib.rs"),
        "pub mod greet;\npub mod util;\n",
    );
    write(
        &root.join("shared/src/util.rs"),
        "pub fn who() -> String { \"world\".to_string() }\n",
    );
    // `super::` must stay relative and `crate::` must pin to the grafted root, not the script
    // root, neither may fall through to bridge dispatch
    write(
        &root.join("shared/src/greet.rs"),
        "use crate::util::who;\npub fn hi() -> String { format!(\"hi {}\", super::util::who()) }\npub fn yo() -> String { format!(\"yo {} {}\", who(), crate::util::who()) }\n",
    );

    write(
        &root.join("app/Cargo.toml"),
        r#"[package]
name = "app"
version = "0.0.0"
edition = "2024"
[dependencies]
shared = { path = "../shared" }
[workspace]
"#,
    );
    // a bare `shared::` from a deep module only works for a real extern crate
    let bin = root.join("app/src/bin/foo.rs");
    write(
        &bin,
        "#!/usr/bin/env rust\nuse shared::greet::{hi, yo};\nmod deep;\nfn main() {\n    println!(\"{}\", hi());\n    println!(\"{}\", yo());\n    deep::go();\n}\n",
    );
    write(
        &root.join("app/src/bin/deep/mod.rs"),
        "use shared::util::who;\npub fn go() { println!(\"deep {}\", who()); }\n",
    );

    (bin, root)
}

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn run_bin(bin: &Path, skip_check: bool) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rust"));
    cmd.arg(bin);
    if skip_check {
        cmd.env("RUSTSCRIPT_SKIP_CHECK", "1");
    }
    cmd.output().expect("failed to launch rustscript")
}

#[test]
fn grafts_local_crate_at_runtime() {
    let (bin, root) = fixture();
    let out = run_bin(&bin, true);
    std::fs::remove_dir_all(&root).unwrap();
    assert!(
        out.status.success(),
        "run failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "hi world\nyo world world\ndeep world\n"
    );
}

#[test]
fn grafts_hyphenated_local_crate() {
    // cargo maps the hyphen to an underscore, so the grafted module must be `my_shared`
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root =
        std::env::temp_dir().join(format!("rustscript_hyphen_{}_{}", std::process::id(), id));
    if root.exists() {
        std::fs::remove_dir_all(&root).unwrap();
    }
    write(
        &root.join("my-shared/Cargo.toml"),
        "[package]\nname = \"my-shared\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[dependencies]\n[workspace]\n",
    );
    write(&root.join("my-shared/src/lib.rs"), "pub mod util;\n");
    write(
        &root.join("my-shared/src/util.rs"),
        "pub fn who() -> String { \"world\".to_string() }\n",
    );
    write(
        &root.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[dependencies]\nmy-shared = { path = \"../my-shared\" }\n[workspace]\n",
    );
    let bin = root.join("app/src/bin/foo.rs");
    write(
        &bin,
        "#!/usr/bin/env rust\nuse my_shared::util::who;\nfn main() { println!(\"hi {}\", who()); }\n",
    );
    let out = run_bin(&bin, true);
    std::fs::remove_dir_all(&root).unwrap();
    assert!(
        out.status.success(),
        "run failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hi world\n");
}

#[test]
fn checks_local_crate_as_path_dep() {
    let (bin, root) = fixture();
    // Only `check` resolves `shared` as a real path dependency. The graft must not land after the
    // `[target."cfg(windows)".dependencies]` table, that makes it `Windows` only.
    let out = Command::new(env!("CARGO_BIN_EXE_rust"))
        .arg("check")
        .arg(&bin)
        .output()
        .expect("failed to launch rustscript");
    std::fs::remove_dir_all(&root).unwrap();
    assert!(
        out.status.success(),
        "rust check failed to resolve the local crate:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
