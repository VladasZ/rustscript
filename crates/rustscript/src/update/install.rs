use std::env::{current_exe, var_os};
use std::fs::{remove_file, rename};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use dirs::home_dir;
use which::which;

pub const BINARY: &str = if cfg!(windows) { "rust.exe" } else { "rust" };

/// Where an update puts the binary.
#[derive(Debug, PartialEq, Eq)]
pub enum Place {
    /// The `bin` folder of this cargo home. Cargo keeps its install list there.
    Cargo(PathBuf),
    /// The folder of the running binary, on a machine with no cargo.
    Beside(PathBuf),
}

impl Place {
    pub fn target(&self) -> PathBuf {
        match self {
            Self::Cargo(home) => home.join("bin").join(BINARY),
            Self::Beside(folder) => folder.join(BINARY),
        }
    }
}

/// A script runs with no cargo on the machine, so the update must not need
/// one either.
pub fn place() -> Result<Place> {
    let home = match var_os("CARGO_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home_dir()
            .context("could not find the home directory")?
            .join(".cargo"),
    };
    let running = current_exe().context("could not find the running rust binary")?;
    place_for(&home, &running)
}

fn place_for(cargo_home: &Path, running: &Path) -> Result<Place> {
    if cargo_home.join("bin").is_dir() {
        return Ok(Place::Cargo(cargo_home.to_path_buf()));
    }
    // A link in a folder on PATH must not get a real file in its place.
    let real = running
        .canonicalize()
        .with_context(|| format!("could not resolve {}", running.display()))?;
    let folder = real
        .parent()
        .with_context(|| format!("{} has no folder", real.display()))?;
    Ok(Place::Beside(folder.to_path_buf()))
}

/// An older copy earlier on PATH keeps winning after an update, so say
/// which file is really being run.
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

/// The installed binary is still untouched here.
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

/// The old binary is kept until the new one is in place, so a failure can put
/// it back.
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

/// `Windows` cannot delete the running binary, so leftovers go on the next
/// run.
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
    use std::fs::{create_dir_all, read_to_string, write};
    use std::path::Path;

    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    use super::{Place, move_aside, old_path, place_for, restore, swap};

    #[test]
    fn a_cargo_home_with_a_bin_folder_takes_the_update() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("cargo");
        create_dir_all(home.join("bin")).unwrap();
        let running = dir.path().join("elsewhere").join("rust");

        assert_eq!(place_for(&home, &running).unwrap(), Place::Cargo(home));
    }

    /// The failure this prevents: `rust update` stops on a machine with no
    /// cargo, where the binary sits in a folder like `~/.local/bin`.
    #[test]
    fn without_cargo_the_update_goes_next_to_the_running_binary() {
        let dir = tempdir().unwrap();
        let folder = dir.path().canonicalize().unwrap().join("local-bin");
        create_dir_all(&folder).unwrap();
        let running = folder.join("rust");
        write(&running, "running").unwrap();

        let place = place_for(&dir.path().join("no-cargo"), &running).unwrap();

        assert_eq!(place, Place::Beside(folder.clone()));
        assert_eq!(place.target(), folder.join(super::BINARY));
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
