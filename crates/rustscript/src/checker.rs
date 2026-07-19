//! Validate that a script is real, valid Rust by building a small cargo
//! project around it and running `cargo check`. This runs only for the
//! `rust check` command, never when a script runs. Results cache by source
//! hash, so an unchanged script rechecks instantly.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, anyhow, bail};

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

/// Compiled script binaries, one per source hash. Kept so an unchanged script
/// runs instantly with no cargo invocation.
fn bin_cache() -> PathBuf {
    cache_root().join("bin")
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
pub fn check(
    script_path: &Path,
    files: &[(PathBuf, String)],
    crate_deps: &[CrateDep],
) -> Result<()> {
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

    write_project(&project, files, crate_deps)?;

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

/// Mirror the script and its module tree into a throwaway cargo project under
/// the cache dir, so `mod` declarations resolve the same way real Rust sees
/// them. Shared by the check gate and the compiled build.
fn write_project(
    project: &Path,
    files: &[(PathBuf, String)],
    crate_deps: &[CrateDep],
) -> Result<()> {
    std::fs::create_dir_all(project)?;
    std::fs::write(project.join("Cargo.toml"), manifest(crate_deps))?;
    for (rel, source) in files {
        let dst = project.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dst, source)?;
    }
    Ok(())
}

/// Compile the script to a native binary and return its cached path. A
/// successful build proves the script is valid Rust, so it also stands in for
/// the check gate. The final binary is cached by source hash for instant
/// re-runs. The one shared target dir is kept so an edit rebuilds only the
/// script crate, but no per-hash target dirs are ever created.
pub fn build(
    script_path: &Path,
    files: &[(PathBuf, String)],
    crate_deps: &[CrateDep],
) -> Result<PathBuf> {
    let hash = hash_files(files, crate_deps);
    let bin = bin_cache().join(format!("{hash:016x}{}", std::env::consts::EXE_SUFFIX));
    if bin.exists() {
        return Ok(bin);
    }

    let project = cache_root().join(format!("{hash:016x}"));
    write_project(&project, files, crate_deps)?;

    let target = cache_root().join("target");
    eprintln!("rust: compiling {}", script_path.display());
    let status = Command::new("cargo")
        .args(["build"])
        .env("CARGO_TARGET_DIR", &target)
        .current_dir(&project)
        .status();
    let status = match status {
        Ok(s) => s,
        Err(e) => bail!("could not run cargo build: {e}"),
    };
    if !status.success() {
        bail!("{} failed to compile", script_path.display());
    }

    let built = target
        .join("debug")
        .join(format!("script{}", std::env::consts::EXE_SUFFIX));
    std::fs::create_dir_all(bin_cache())?;
    // Copy to a per-process temp path then rename, so a concurrent run never
    // execs a half-written binary. copy carries the executable bit over.
    let tmp = bin_cache().join(format!(".{hash:016x}.{}", std::process::id()));
    std::fs::copy(&built, &tmp)
        .map_err(|e| anyhow!("cannot copy built binary {}: {e}", built.display()))?;
    match std::fs::rename(&tmp, &bin) {
        Ok(()) => {}
        Err(e) => {
            // A concurrent build may have placed the same binary first. If it
            // did, its bytes are identical, so reuse it and drop our temp.
            if let Err(rm) = std::fs::remove_file(&tmp) {
                eprintln!("rust: could not remove temp binary {}: {rm}", tmp.display());
            }
            if !bin.exists() {
                return Err(anyhow!("cannot place binary {}: {e}", bin.display()));
            }
        }
    }
    Ok(bin)
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
tokio = { version = "1", features = ["full"] }
reqwest = { version = "0.12", features = ["json", "rustls-tls", "blocking", "cookies"], default-features = false }

[target."cfg(windows)".dependencies]
winreg = "0.56"
windows-service = "0.8"
wmi = "0.18"
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
