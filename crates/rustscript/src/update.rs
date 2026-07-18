use std::env::current_exe;
use std::fs::{remove_file, rename};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use which::which;

use crate::build_info::{GIT_COMMIT, short_commit};

const REPOSITORY: &str = "https://github.com/VladasZ/rustscript";

pub fn update() -> Result<()> {
    let rust_bin = which("rust").or_else(|_| current_exe())?;
    if cfg!(windows) {
        cleanup_stale_binaries(&rust_bin);
    }

    let remote = remote_head()?;
    if is_current(GIT_COMMIT, &remote) {
        println!(
            "rustscript is already up to date ({})",
            short_commit(&remote)
        );
        return Ok(());
    }

    println!(
        "updating rustscript from {} to {}",
        short_commit(GIT_COMMIT),
        short_commit(&remote)
    );

    let moved = if cfg!(windows) && rust_bin.exists() {
        Some(move_aside(&rust_bin)?)
    } else {
        None
    };

    if let Err(error) = install_latest(&remote) {
        if let Some(old) = &moved
            && let Err(restore_error) = restore_binary(old, &rust_bin)
        {
            bail!("{error:#}; restoring the previous rust binary also failed: {restore_error:#}");
        }
        return Err(error);
    }

    if !rust_bin.exists() {
        if let Some(old) = &moved {
            restore_binary(old, &rust_bin)?;
        }
        bail!(
            "cargo reported success but did not install {}",
            rust_bin.display()
        );
    }

    if let Some(old) = &moved
        && let Err(error) = remove_file(old)
    {
        eprintln!(
            "rust update: the previous binary remains at {} and will be removed next time: {error}",
            old.display()
        );
    }

    println!("updated rustscript to {}", short_commit(&remote));
    Ok(())
}

fn is_current(installed: &str, remote: &str) -> bool {
    installed == remote
}

fn remote_head() -> Result<String> {
    let output = Command::new("git")
        .args(["ls-remote", REPOSITORY, "HEAD"])
        .output()
        .context("could not query the RustScript repository")?;
    if !output.status.success() {
        bail!("git ls-remote exited with {}", output.status);
    }
    let stdout = String::from_utf8(output.stdout).context("git returned a non-UTF-8 commit")?;
    let commit = stdout
        .split_whitespace()
        .next()
        .context("git returned no RustScript HEAD commit")?;
    if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("git returned an invalid RustScript HEAD commit: {commit}");
    }
    Ok(commit.to_string())
}

fn install_latest(commit: &str) -> Result<()> {
    let status = Command::new("cargo")
        .args([
            "install",
            "--git",
            REPOSITORY,
            "--rev",
            commit,
            "rustscript",
        ])
        .status()
        .context("could not start cargo install")?;
    if !status.success() {
        bail!("cargo install exited with {status}");
    }
    Ok(())
}

fn old_path(target: &Path, index: usize) -> PathBuf {
    let suffix = if index == 0 {
        ".old".to_string()
    } else {
        format!(".old{index}")
    };
    PathBuf::from(format!("{}{suffix}", target.display()))
}

fn cleanup_stale_binaries(target: &Path) {
    for index in 0..100 {
        let old = old_path(target, index);
        if !old.exists() {
            continue;
        }
        if let Err(error) = remove_file(&old) {
            eprintln!(
                "rust update: could not remove stale binary {}: {error}",
                old.display()
            );
        }
    }
}

fn move_aside(target: &Path) -> Result<PathBuf> {
    let mut last_error = None;
    for index in 0..100 {
        let old = old_path(target, index);
        if old.exists()
            && let Err(error) = remove_file(&old)
        {
            last_error = Some(error);
            continue;
        }
        match rename(target, &old) {
            Ok(()) => return Ok(old),
            Err(error) => last_error = Some(error),
        }
    }
    let detail = last_error.map_or_else(|| "no free backup name".to_string(), |e| e.to_string());
    bail!("could not move {} aside: {detail}", target.display())
}

fn restore_binary(old: &Path, target: &Path) -> Result<()> {
    if target.exists() {
        remove_file(target)
            .with_context(|| format!("could not remove failed update at {}", target.display()))?;
    }
    rename(old, target).with_context(|| {
        format!(
            "could not restore previous binary from {} to {}",
            old.display(),
            target.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use super::{is_current, move_aside, old_path, restore_binary};

    #[test]
    fn only_the_exact_remote_commit_is_current() {
        let commit = "4ea5a27a1ffa91d6cbaecb9576f80fab0c69990d";
        assert!(is_current(commit, commit));
        assert!(!is_current(&format!("{commit}-dirty"), commit));
        assert!(!is_current(
            "40043615996fbb085e7f708c4508b5a517889f37",
            commit
        ));
    }

    #[test]
    fn old_paths_are_stable() {
        let target = Path::new("/tmp/rust");
        assert_eq!(old_path(target, 0), Path::new("/tmp/rust.old"));
        assert_eq!(old_path(target, 2), Path::new("/tmp/rust.old2"));
    }

    #[test]
    fn move_and_restore_preserve_the_previous_binary() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("rust");
        fs::write(&target, "working").unwrap();

        let old = move_aside(&target).unwrap();
        assert!(!target.exists());
        assert_eq!(fs::read_to_string(&old).unwrap(), "working");

        fs::write(&target, "failed update").unwrap();
        restore_binary(&old, &target).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "working");
        assert!(!old.exists());
    }
}
