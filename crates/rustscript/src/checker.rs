//! Validate that a script is real, valid Rust by building a small cargo
//! project around it and running `cargo check`. This runs only for the
//! `rust check` command, never when a script runs. Results cache by source
//! hash, so an unchanged script rechecks instantly.

use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

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

/// A cache entry unused for this long is removed by the sweep that runs after
/// every check and build. Interpreted runs never touch the cache.
const GC_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Mark a cache file as used now, so the age sweep keeps its entry.
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

/// Drop cache entries unused for `GC_MAX_AGE`. A project dir's use time is its
/// `.checked` stamp, refreshed on every hit, with the mirrored `Cargo.toml` as
/// the fallback for a project that never earned a stamp. A compiled binary's
/// use time is its own mtime. The shared `target` dir is never removed, cargo
/// reuses it across entries and rebuilding it is the cost the cache exists to
/// avoid.
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

/// Compiled binaries and leftover temp copies, one file per entry.
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

/// Missing metadata and a clock that ran backwards both read as fresh, so the
/// sweep only removes entries whose age is positively known.
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

/// `files` are the script's sources: path relative to the script directory
/// and content, the root script first under its real name. The layout is mirrored
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
        touch(&stamp);
        sweep();
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
    sweep();
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
        touch(&bin);
        sweep();
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
    sweep();
    Ok(bin)
}

/// Scripts use plain `std`. A few common crates are always available so a
/// script can `use` them without declaring anything.
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
tokio = { version = "1", features = ["full"] }
reqwest = { version = "0.12", features = ["json", "rustls-tls", "blocking", "cookies"], default-features = false }

[target."cfg(windows)".dependencies]
winreg = "0.56"
windows-service = "0.8"
wmi = "0.18"
"#;

/// The cargo project manifest, the fixed dependency set plus one `path` entry
/// per local crate the script grafts in. The empty `[workspace]` detaches the
/// project from any workspace above the cache directory. The bin path is the
/// root script's real name, so rustc diagnostics name the script the user ran,
/// not a generic `main.rs`.
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
        // Each graft gets its own `[dependencies.name]` table header, not a
        // bare `name = ..` line. MANIFEST ends with the
        // `[target."cfg(windows)".dependencies]` table, so a bare key appended
        // after it would join that table and make the crate Windows only,
        // unresolved on every other platform. The explicit header puts it back
        // in the all-target dependencies no matter what section precedes it.
        out.push_str(&format!("\n[dependencies.{}]\npath = {dir:?}\n", dep.name));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A grafted `path` crate must be an all-target dependency. A regression
    /// once appended it as a bare key after the Windows target table, so it
    /// became Windows only and `use shared::..` failed to compile on macOS and
    /// Linux while `rust run` still worked through the interpreter.
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
        let old = now - GC_MAX_AGE - Duration::from_secs(60 * 60 * 24);

        project_entry(root, "stale", true, old);
        project_entry(root, "fresh", true, now);
        // A project that never earned a stamp, a failed check for example,
        // must still age out through its mirrored Cargo.toml.
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

    /// The bin path must be the script's real name, so a rustc diagnostic from
    /// the mirrored project points at `notes.rs`, not a generic `main.rs`.
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
