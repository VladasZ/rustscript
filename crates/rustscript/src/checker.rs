//! The `cargo check` gate behind `rust check`. Never runs when a script
//! runs. Results cache by source hash.

use hex::encode;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use anyhow::{Result, anyhow, bail};

use crate::loader::CrateDep;

fn cache_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(dir).join("rustscript");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".cache/rustscript");
    }
    // `dirs` knows the per user cache dir even on `Windows`. The old fallback
    // was the shared `/tmp`, so `cmp` would exec a binary from a path any
    // other user could create first.
    if let Some(dir) = dirs::cache_dir() {
        return dir.join("rustscript");
    }
    std::env::temp_dir().join("rustscript")
}

/// Compiled script binaries, one per source hash.
fn bin_cache() -> PathBuf {
    cache_root().join("bin")
}

/// Entries unused this long are swept after every check and build.
const GC_MAX_AGE: Duration = Duration::from_hours(720);

fn touch(path: &Path) {
    let refreshed = File::options()
        .append(true)
        .open(path)
        .and_then(|f| f.set_modified(SystemTime::now()));
    if let Err(e) = refreshed {
        eprintln!(
            "rust: could not refresh cache stamp {}: {e}",
            path.display()
        );
    }
}

/// A project's use time is its `.checked` stamp, or the mirrored `Cargo.toml`
/// when it never earned one. The shared `target` dir is never removed,
/// rebuilding it is the cost the cache exists to avoid.
fn sweep() {
    sweep_root(&cache_root(), SystemTime::now());
}

fn sweep_root(root: &Path, now: SystemTime) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) => {
            if e.kind() != ErrorKind::NotFound {
                eprintln!("rust: could not sweep cache {}: {e}", root.display());
            }
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name() == "target" {
            continue;
        }
        if entry.file_name() == "bin" {
            sweep_bin(&path, now);
            continue;
        }
        if !path.is_dir() {
            continue;
        }
        let used = mtime(&path.join(".checked"))
            .or_else(|| mtime(&path.join("Cargo.toml")))
            .or_else(|| mtime(&path));
        if is_expired(used, now)
            && let Err(e) = std::fs::remove_dir_all(&path)
        {
            eprintln!(
                "rust: could not remove stale cache entry {}: {e}",
                path.display()
            );
        }
    }
}

fn sweep_bin(dir: &Path, now: SystemTime) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            if e.kind() != ErrorKind::NotFound {
                eprintln!("rust: could not sweep cache {}: {e}", dir.display());
            }
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_expired(mtime(&path), now)
            && let Err(e) = std::fs::remove_file(&path)
        {
            eprintln!(
                "rust: could not remove stale cache binary {}: {e}",
                path.display()
            );
        }
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Missing metadata and a clock that ran backwards read as fresh, so only a
/// known age is swept.
fn is_expired(used: Option<SystemTime>, now: SystemTime) -> bool {
    let Some(used) = used else { return false };
    now.duration_since(used).is_ok_and(|age| age > GC_MAX_AGE)
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

/// `files` are mirrored into the cache project so `mod` resolves the same
/// way. `crate_deps` join it as path dependencies.
pub fn check(
    script_path: &Path,
    files: &[(PathBuf, String)],
    crate_deps: &[CrateDep],
) -> Result<()> {
    if std::env::var_os("RUSTSCRIPT_SKIP_CHECK").is_some() {
        return Ok(());
    }
    let hash = hash_files(files, crate_deps);
    let project = cache_root().join(hash.clone());
    let stamp = project.join(".checked");
    if stamp.exists() {
        touch(&stamp);
        sweep();
        return Ok(());
    }

    write_project(&project, files, crate_deps)?;

    // One shared target dir, or every source hash recompiles the whole dep
    // set and piles up gigabytes.
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
    sweep();
    Ok(())
}

/// Shared by the check gate and the compiled build.
fn write_project(
    project: &Path,
    files: &[(PathBuf, String)],
    crate_deps: &[CrateDep],
) -> Result<()> {
    std::fs::create_dir_all(project)?;
    let root = files.first().map(|(rel, _)| rel.as_path());
    std::fs::write(project.join("Cargo.toml"), manifest(root, crate_deps))?;
    for (rel, source) in files {
        let dst = project.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dst, source)?;
    }
    Ok(())
}

/// A successful build also stands in for the check gate. The binary is
/// cached by source hash, no per hash target dirs are ever created.
pub fn build(
    script_path: &Path,
    files: &[(PathBuf, String)],
    crate_deps: &[CrateDep],
) -> Result<PathBuf> {
    let hash = hash_files(files, crate_deps);
    let bin = bin_cache().join(format!("{hash}{}", std::env::consts::EXE_SUFFIX));
    if bin.exists() {
        touch(&bin);
        sweep();
        return Ok(bin);
    }

    let project = cache_root().join(hash.clone());
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
    // Copy to a temp path then rename, so a concurrent run never execs a half
    // written binary.
    let tmp = bin_cache().join(format!(".{hash}.{}", std::process::id()));
    std::fs::copy(&built, &tmp)
        .map_err(|e| anyhow!("cannot copy built binary {}: {e}", built.display()))?;
    match std::fs::rename(&tmp, &bin) {
        Ok(()) => {}
        Err(e) => {
            // A concurrent build may have placed the identical binary first.
            if let Err(rm) = std::fs::remove_file(&tmp) {
                eprintln!("rust: could not remove temp binary {}: {rm}", tmp.display());
            }
            if !bin.exists() {
                return Err(anyhow!("cannot place binary {}: {e}", bin.display()));
            }
        }
    }
    sweep();
    Ok(bin)
}

/// Always available, so a script can `use` them without declaring anything.
const MANIFEST: &str = r#"[dependencies]
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
sha2 = "0.11"
ctrlc = "3"
tempfile = "3"
jsonwebtoken = { version = "10", features = ["rust_crypto"] }
lopdf = "0.44"
xmltree = { version = "0.12", features = ["attribute-order"] }
ratatui = { version = "0.30", default-features = false }
crossterm = "0.29"
terminal-light = "1.9"
tokio = { version = "1", features = ["full"] }
reqwest = { version = "0.12", features = ["json", "rustls-tls", "blocking", "cookies"], default-features = false }

[target."cfg(windows)".dependencies]
winreg = "0.56"
windows-service = "0.8"
wmi = "0.18"
"#;

/// The empty `[workspace]` detaches the project from any workspace above the
/// cache dir. The bin path is the script's real name, so diagnostics name
/// the script the user ran.
fn manifest(root: Option<&Path>, crate_deps: &[CrateDep]) -> String {
    let root = root.unwrap_or(Path::new("main.rs")).to_string_lossy();
    let mut out = format!(
        r#"[package]
name = "script"
version = "0.0.0"
edition = "2024"

[[bin]]
name = "script"
path = {root:?}

{MANIFEST}"#
    );
    for dep in crate_deps {
        let dir = dep.dir.to_string_lossy();
        // An explicit `[dependencies.name]` header, because `MANIFEST` ends
        // with the `[target."cfg(windows)".dependencies]` table and a bare key
        // would join it and make the crate `Windows` only.
        out.push_str(&format!("\n[dependencies.{}]\npath = {dir:?}\n", dep.name));
    }
    out.push_str("\n[workspace]\n");
    out
}

/// The key covers the `MANIFEST` and the compiler too, not only the sources.
/// Without that a `rust update` left old stamps vouching for sources the new
/// dep set might reject. Sha256 because `DefaultHasher` is not stable across
/// releases.
fn hash_files(files: &[(PathBuf, String)], crate_deps: &[CrateDep]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(MANIFEST.as_bytes());
    hasher.update(crate::build_info::version().as_bytes());
    for (rel, source) in files {
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update(source.as_bytes());
    }
    // A grafted crate change must re-trigger the check.
    for dep in crate_deps {
        hasher.update(dep.name.as_bytes());
        for (rel, source) in &dep.files {
            hasher.update(rel.to_string_lossy().as_bytes());
            hasher.update(source.as_bytes());
        }
    }
    // Half the digest is still 128 bits and keeps the dir names readable.
    encode(&hasher.finalize()[..16])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A regression once appended the graft after the `Windows` target table,
    /// so `use shared::..` failed to compile off `Windows`.
    #[test]
    fn graft_dep_is_all_target_not_windows_only() {
        let dep = CrateDep {
            name: "shared".to_string(),
            dir: PathBuf::from("/tmp/shared"),
            files: Vec::new(),
        };
        let text = manifest(Some(Path::new("notes.rs")), &[dep]);
        let value: toml::Value = toml::from_str(&text)
            .unwrap_or_else(|e| panic!("manifest must be valid TOML: {e}\n{text}"));

        let all_target = value
            .get("dependencies")
            .and_then(|d| d.get("shared"))
            .is_some();
        assert!(
            all_target,
            "shared must be an all-target dependency:\n{text}"
        );

        let windows_only = value
            .get("target")
            .and_then(|t| t.get("cfg(windows)"))
            .and_then(|c| c.get("dependencies"))
            .and_then(|d| d.get("shared"))
            .is_some();
        assert!(
            !windows_only,
            "shared must not be a Windows only dependency:\n{text}"
        );
    }

    fn source(name: &str, body: &str) -> Vec<(PathBuf, String)> {
        vec![(PathBuf::from(name), body.to_string())]
    }

    /// Anything that could change cargo's answer belongs in the key.
    #[test]
    fn the_cache_key_separates_different_inputs() {
        let base = hash_files(&source("a.rs", "fn main() {}"), &[]);
        assert_eq!(base.len(), 32, "128 bits rendered as hex");
        assert!(base.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(base, hash_files(&source("a.rs", "fn main() {}"), &[]));

        assert_ne!(base, hash_files(&source("a.rs", "fn main() { }"), &[]));
        assert_ne!(base, hash_files(&source("b.rs", "fn main() {}"), &[]));

        let dep = CrateDep {
            name: "shared".to_string(),
            dir: PathBuf::from("/tmp/shared"),
            files: source("lib.rs", "pub fn helper() {}"),
        };
        let with_dep = hash_files(&source("a.rs", "fn main() {}"), &[dep]);
        assert_ne!(base, with_dep);

        // A grafted crate change must re-trigger the check.
        let changed_dep = CrateDep {
            name: "shared".to_string(),
            dir: PathBuf::from("/tmp/shared"),
            files: source("lib.rs", "pub fn helper() -> u8 { 0 }"),
        };
        let with_changed = hash_files(&source("a.rs", "fn main() {}"), &[changed_dep]);
        assert_ne!(with_dep, with_changed);
    }

    fn set_mtime(path: &Path, to: SystemTime) {
        File::options()
            .append(true)
            .open(path)
            .unwrap()
            .set_modified(to)
            .unwrap();
    }

    fn project_entry(root: &Path, name: &str, stamped: bool, used: SystemTime) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();
        set_mtime(&dir.join("Cargo.toml"), used);
        if stamped {
            std::fs::write(dir.join(".checked"), "").unwrap();
            set_mtime(&dir.join(".checked"), used);
        }
    }

    #[test]
    fn sweep_removes_stale_entries_and_never_the_target_dir() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let now = SystemTime::now();
        let old = now - GC_MAX_AGE - Duration::from_hours(24);

        project_entry(root, "stale", true, old);
        project_entry(root, "fresh", true, now);
        // A project that never earned a stamp must still age out.
        project_entry(root, "unstamped", false, old);
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("aaaa"), "x").unwrap();
        set_mtime(&bin.join("aaaa"), old);
        std::fs::write(bin.join("bbbb"), "x").unwrap();

        sweep_root(root, now);

        assert!(!root.join("stale").exists(), "stale project must go");
        assert!(
            !root.join("unstamped").exists(),
            "unstamped project must go"
        );
        assert!(root.join("fresh").exists(), "fresh project must stay");
        assert!(root.join("target/debug").exists(), "target must never go");
        assert!(!bin.join("aaaa").exists(), "stale binary must go");
        assert!(bin.join("bbbb").exists(), "fresh binary must stay");
    }

    /// A diagnostic must point at `notes.rs`, not `main.rs`.
    #[test]
    fn manifest_bin_path_is_the_real_script_name() {
        let text = manifest(Some(Path::new("notes.rs")), &[]);
        let value: toml::Value = toml::from_str(&text)
            .unwrap_or_else(|e| panic!("manifest must be valid TOML: {e}\n{text}"));
        let path = value
            .get("bin")
            .and_then(|b| b.get(0))
            .and_then(|b| b.get("path"))
            .and_then(|p| p.as_str());
        assert_eq!(path, Some("notes.rs"), "manifest was:\n{text}");
    }
}
