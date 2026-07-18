use std::env::current_exe;
use std::fs::{remove_file, rename};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use which::which;

const REPOSITORY: &str = "https://github.com/VladasZ/rustscript";

type Version = (u64, u64, u64);

struct Release {
    tag: String,
    version: Version,
}

pub fn update() -> Result<()> {
    let rust_bin = which("rust").or_else(|_| current_exe())?;
    if cfg!(windows) {
        cleanup_stale_binaries(&rust_bin);
    }

    let installed = parse_version(env!("CARGO_PKG_VERSION"))
        .context("the installed rustscript reports an unreadable version")?;
    let latest = latest_release()?;

    if installed >= latest.version {
        println!(
            "rustscript is already up to date (v{})",
            format_version(installed)
        );
        return Ok(());
    }

    println!(
        "updating rustscript from v{} to {}",
        format_version(installed),
        latest.tag
    );

    let moved = if cfg!(windows) && rust_bin.exists() {
        Some(move_aside(&rust_bin)?)
    } else {
        None
    };

    if let Err(error) = install_release(&latest.tag) {
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

    println!("updated rustscript to {}", latest.tag);
    Ok(())
}

fn format_version((major, minor, patch): Version) -> String {
    format!("{major}.{minor}.{patch}")
}

fn parse_version(tag: &str) -> Option<Version> {
    let tag = tag.strip_prefix('v').unwrap_or(tag);
    // A prerelease is never an update target, and a moving tag like v0.2 has no
    // patch component, so both fail this parse on purpose.
    if tag.contains('-') {
        return None;
    }
    let mut parts = tag.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn latest_release() -> Result<Release> {
    let output = Command::new("git")
        .args(["ls-remote", "--tags", REPOSITORY])
        .output()
        .context("could not query the RustScript repository")?;
    if !output.status.success() {
        bail!("git ls-remote exited with {}", output.status);
    }
    let stdout = String::from_utf8(output.stdout).context("git returned non-UTF-8 tags")?;
    newest_release(&stdout).context("the RustScript repository has no released version yet")
}

fn newest_release(ls_remote: &str) -> Option<Release> {
    ls_remote
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .filter_map(|reference| reference.strip_prefix("refs/tags/"))
        // An annotated tag also lists a peeled entry for the object it points at.
        .map(|tag| tag.trim_end_matches("^{}"))
        .filter_map(|tag| {
            Some(Release {
                tag: tag.to_string(),
                version: parse_version(tag)?,
            })
        })
        .max_by_key(|release| release.version)
}

fn install_release(tag: &str) -> Result<()> {
    let status = Command::new("cargo")
        .args(["install", "--git", REPOSITORY, "--tag", tag, "rustscript"])
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

    use super::{move_aside, newest_release, old_path, parse_version, restore_binary};

    #[test]
    fn versions_parse_with_and_without_the_v_prefix() {
        assert_eq!(parse_version("v0.2.1"), Some((0, 2, 1)));
        assert_eq!(parse_version("0.2.1"), Some((0, 2, 1)));
        assert_eq!(parse_version("v10.20.30"), Some((10, 20, 30)));
    }

    #[test]
    fn moving_tags_and_prereleases_are_not_update_targets() {
        assert_eq!(parse_version("v0.2"), None);
        assert_eq!(parse_version("v0.2.0-rc.1"), None);
        assert_eq!(parse_version("v0.2.0.1"), None);
        assert_eq!(parse_version("main"), None);
        assert_eq!(parse_version("vX.Y.Z"), None);
    }

    #[test]
    fn the_newest_release_wins_over_peeled_and_moving_tags() {
        let ls_remote = r"1111111111111111111111111111111111111111	refs/tags/v0.1.0
2222222222222222222222222222222222222222	refs/tags/v0.2.0
2222222222222222222222222222222222222222	refs/tags/v0.2.0^{}
3333333333333333333333333333333333333333	refs/tags/v0.2
4444444444444444444444444444444444444444	refs/tags/v0.3.0-rc.1
5555555555555555555555555555555555555555	refs/tags/v0.10.0
";
        let newest = newest_release(ls_remote).unwrap();
        assert_eq!(newest.tag, "v0.10.0");
        assert_eq!(newest.version, (0, 10, 0));
    }

    #[test]
    fn a_repository_without_releases_has_no_newest_release() {
        let ls_remote = "3333333333333333333333333333333333333333	refs/tags/v0.2\n";
        assert!(newest_release(ls_remote).is_none());
        assert!(newest_release("").is_none());
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
