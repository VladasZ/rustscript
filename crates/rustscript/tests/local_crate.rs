//! A script that lives in a cargo crate can use a local `path` dependency
//! crate. The interpreter grafts that crate in from source so `use shared::..`
//! resolves at runtime, and the checker adds it as a real path dependency so
//! `cargo check` also passes, including a bare `shared::` from a deep module.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use pretty_assertions::assert_eq;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Lay out a crate `app` with a bin that uses a sibling `shared` path crate,
/// and return the bin path. `shared` has no external deps so the check is fast.
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
    // A sibling module reaches another with `super::`, which must stay relative
    // so it resolves both as a real crate and as a grafted module.
    write(
        &root.join("shared/src/greet.rs"),
        "pub fn hi() -> String { format!(\"hi {}\", super::util::who()) }\n",
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
    // The bin uses `shared::` at the root, and a deep module `deep` uses a bare
    // `shared::` too, which only a real extern crate allows.
    let bin = root.join("app/src/bin/foo.rs");
    write(
        &bin,
        "#!/usr/bin/env rust\nuse shared::greet::hi;\nmod deep;\nfn main() {\n    println!(\"{}\", hi());\n    deep::go();\n}\n",
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
    cmd.arg("run").arg(bin);
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
        "hi world\ndeep world\n"
    );
}

#[test]
fn grafts_hyphenated_local_crate() {
    // A hyphenated package name like `my-shared` is `my_shared` in Rust code.
    // Cargo maps the hyphen to an underscore for the crate identifier, so the
    // grafted module has to be named `my_shared`, not the raw dependency key.
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
    // The `check` command, not `run`. Only `check` builds a cargo project and
    // resolves `shared` as a real path dependency, so `run` never exercised
    // this. A manifest bug once appended the graft as a bare key after the
    // `[target."cfg(windows)".dependencies]` table, which made it Windows only,
    // so `cargo check` dropped it and `use shared::..` failed off Windows.
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
