use std::env::var_os;
use std::fs::{remove_file, rename};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use dirs::home_dir;
use which::which;

pub const BINARY: &str = if cfg!(windows) { "rust.exe" } else { "rust" };

/// Where cargo keeps its installed binaries and its install records.
/// RustScript needs cargo anyway to check and build scripts, so a missing
/// cargo directory is a broken setup rather than a case to work around.
pub fn cargo_home() -> Result<PathBuf> {
    let home = match var_os("CARGO_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home_dir()
            .context("could not find the home directory")?
            .join(".cargo"),
    };
    let bin = home.join("bin");
    if !bin.is_dir() {
        bail!(
            "{} does not exist, RustScript needs a working cargo installation",
            bin.display()
        );
    }
    Ok(home)
}

/// An older copy earlier on PATH keeps winning after a successful update, so
/// say which file is really being run.
pub fn warn_if_shadowed(target: &Path) {
    let Ok(found) = which("rust") else {
        return;
    };
    if same_file(&found, target) {
        return;
    }
    eprintln!("warning: the rust on your PATH is {}", found.display());
    eprintln!(
        "warning: it shadows the updated {}, remove it or fix the PATH order",
        target.display()
    );
}

fn same_file(left: &Path, right: &Path) -> bool {
    let resolve = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    resolve(left) == resolve(right)
}

/// Prove the downloaded binary runs and is the version it claims, while the
/// installed one is still untouched.
pub fn verify(binary: &Path, tag: &str) -> Result<()> {
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .with_context(|| format!("the downloaded binary at {} did not run", binary.display()))?;
    if !output.status.success() {
        bail!(
            "the downloaded binary exited with {} on --version",
            output.status
        );
    }
    let reported = String::from_utf8_lossy(&output.stdout);
    let expected = format!("rustscript {}", tag.strip_prefix('v').unwrap_or(tag));
    if !reported.trim_start().starts_with(&expected) {
        bail!(
            "the downloaded binary reports `{}`, expected {expected}",
            reported.trim()
        );
    }
    Ok(())
}

/// Put `staged` in place of `target`, keeping the old binary until the new one
/// is in place so a failure can put it back.
pub fn swap(staged: &Path, target: &Path) -> Result<()> {
    let moved = if target.exists() {
        Some(move_aside(target)?)
    } else {
        None
    };

    if let Err(error) = rename(staged, target) {
        let failure = format!(
            "could not move {} to {}: {error}",
            staged.display(),
            target.display()
        );
        if let Some(old) = &moved
            && let Err(restore_error) = restore(old, target)
        {
            bail!("{failure}; restoring the previous rust binary also failed: {restore_error:#}");
        }
        bail!("{failure}");
    }

    if let Some(old) = &moved
        && let Err(error) = remove_file(old)
    {
        eprintln!(
            "rust update: the previous binary remains at {} and will be removed next time: {error}",
            old.display()
        );
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

/// Windows cannot delete the running binary, so its leftovers go on the next
/// run instead.
pub fn cleanup_stale_binaries(target: &Path) {
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

pub fn move_aside(target: &Path) -> Result<PathBuf> {
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

pub fn restore(old: &Path, target: &Path) -> Result<()> {
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
    use std::fs::{read_to_string, write};
    use std::path::Path;

    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    use super::{move_aside, old_path, restore, swap};

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
        write(&target, "working").unwrap();

        let old = move_aside(&target).unwrap();
        assert!(!target.exists());
        assert_eq!(read_to_string(&old).unwrap(), "working");

        write(&target, "failed update").unwrap();
        restore(&old, &target).unwrap();
        assert_eq!(read_to_string(&target).unwrap(), "working");
        assert!(!old.exists());
    }

    #[test]
    fn a_swap_replaces_the_binary_and_leaves_no_backup() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("rust");
        let staged = dir.path().join("rust.new");
        write(&target, "old").unwrap();
        write(&staged, "new").unwrap();

        swap(&staged, &target).unwrap();

        assert_eq!(read_to_string(&target).unwrap(), "new");
        assert!(!staged.exists());
        assert!(!old_path(&target, 0).exists());
    }

    #[test]
    fn a_swap_works_when_nothing_is_installed_yet() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("rust");
        let staged = dir.path().join("rust.new");
        write(&staged, "new").unwrap();

        swap(&staged, &target).unwrap();

        assert_eq!(read_to_string(&target).unwrap(), "new");
    }
}
