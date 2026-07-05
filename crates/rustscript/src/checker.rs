//! Validate that a script is real, valid Rust by building a small cargo
//! project around it and running `cargo check`. This runs only for the
//! `rust check` command, never when a script runs. Results cache by source
//! hash, so an unchanged script rechecks instantly.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};

use crate::loader::CrateDep;

fn cache_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME") {
        PathBuf::from(dir).join("rustscript")
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".cache/rustscript")
    } else {
        std::env::temp_dir().join("rustscript")
    }
}

pub fn clean() -> Result<()> {
    let root = cache_root();
    if root.exists() {
        std::fs::remove_dir_all(&root)?;
        println!("cleared {}", root.display());
    } else {
        println!("nothing to clean");
    }
    Ok(())
}

/// `files` are the script's sources: path relative to the script directory
/// and content, the root script first as `main.rs`. The layout is mirrored
/// into the cache project so `mod` declarations resolve the same way.
/// `crate_deps` are local `path` crates the script uses; they join the cargo
/// project as path dependencies so `use crate_name::..` resolves.
pub fn check(script_path: &Path, files: &[(PathBuf, String)], crate_deps: &[CrateDep]) -> Result<()> {
    // Tests and fast iteration can skip the compile gate.
    if std::env::var_os("RUSTSCRIPT_SKIP_CHECK").is_some() {
        return Ok(());
    }
    let hash = hash_files(files, crate_deps);
    let project = cache_root().join(format!("{hash:016x}"));
    let stamp = project.join(".checked");
    if stamp.exists() {
        return Ok(());
    }

    std::fs::create_dir_all(&project)?;
    std::fs::write(&project.join("Cargo.toml"), manifest(crate_deps))?;
    for (rel, source) in files {
        let dst = project.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dst, source)?;
    }

    // One shared target dir across all cache projects. Without it every source
    // hash gets its own target and recompiles the whole fixed dep set from
    // scratch, which is slow and piles up gigabytes of duplicate builds.
    let output = Command::new("cargo")
        .args(["check", "--quiet"])
        .env("CARGO_TARGET_DIR", cache_root().join("target"))
        .current_dir(&project)
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => bail!("could not run cargo check: {e}"),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "{} is not valid Rust:\n{}",
            script_path.display(),
            stderr.trim_end()
        );
    }

    std::fs::write(&stamp, "")?;
    Ok(())
}

/// Scripts use plain `std`. A few common crates are always available so a
/// script can `use` them without declaring anything.
const MANIFEST: &str = r#"[package]
name = "script"
version = "0.0.0"
edition = "2024"

[[bin]]
name = "script"
path = "main.rs"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
ureq = { version = "3", features = ["cookies"] }
regex = "1"
which = "8"
rand = "0.10"
glob = "0.3"
chrono = "0.4"
dirs = "6"
toml = "1"
serde_yaml = "0.9"
colored = "3"
base64 = "0.22"
hex = "0.4"
ctrlc = "3"
tempfile = "3"
jsonwebtoken = { version = "10", features = ["rust_crypto"] }
"#;

/// The cargo project manifest, the fixed dependency set plus one `path` entry
/// per local crate the script grafts in. The empty `[workspace]` detaches the
/// project from any workspace above the cache directory.
fn manifest(crate_deps: &[CrateDep]) -> String {
    let mut out = String::from(MANIFEST);
    for dep in crate_deps {
        let dir = dep.dir.to_string_lossy();
        out.push_str(&format!("{} = {{ path = {dir:?} }}\n", dep.name));
    }
    out.push_str("\n[workspace]\n");
    out
}

fn hash_files(files: &[(PathBuf, String)], crate_deps: &[CrateDep]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (rel, source) in files {
        rel.hash(&mut hasher);
        source.hash(&mut hasher);
    }
    // A change in any grafted crate must re-trigger the check for its users.
    for dep in crate_deps {
        dep.name.hash(&mut hasher);
        for (rel, source) in &dep.files {
            rel.hash(&mut hasher);
            source.hash(&mut hasher);
        }
    }
    hasher.finish()
}
