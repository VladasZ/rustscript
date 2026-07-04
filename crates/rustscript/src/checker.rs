//! Validate that a script is real, valid Rust by building a small cargo
//! project around it and running `cargo check`. Results cache by source hash,
//! so an unchanged script skips the check and starts fast.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};

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

pub fn check(script_path: &Path, source: &str) -> Result<()> {
    // Tests and fast iteration can skip the compile gate.
    if std::env::var_os("RUSTSCRIPT_SKIP_CHECK").is_some() {
        return Ok(());
    }
    let hash = hash_source(source);
    let project = cache_root().join(format!("{hash:016x}"));
    let stamp = project.join(".checked");
    if stamp.exists() {
        return Ok(());
    }

    std::fs::create_dir_all(&project)?;
    std::fs::write(&project.join("Cargo.toml"), MANIFEST)?;
    std::fs::write(&project.join("main.rs"), source)?;

    let output = Command::new("cargo")
        .args(["check", "--quiet"])
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

[workspace]
"#;

fn hash_source(source: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}
