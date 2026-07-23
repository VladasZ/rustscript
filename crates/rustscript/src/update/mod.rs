mod asset;
mod install;
mod records;
mod release;

use std::env::consts::{ARCH, OS};
use std::fs::remove_file;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use asset::{Asset, download, download_url, extract, fetch_text, verify_checksum};
use install::{
    BINARY, cargo_home, cleanup_stale_binaries, move_aside, restore, swap, verify, warn_if_shadowed,
};
use records::{install_key, record};
use release::{
    INSTALLED, PACKAGE, REPOSITORY, Release, exact, fetch_tags, format_version, installed_version,
    newest,
};

struct Request {
    version: Option<String>,
    from_source: bool,
}

impl Request {
    fn parse(args: &[String]) -> Result<Self> {
        let mut version = None;
        let mut from_source = false;
        for arg in args {
            match arg.as_str() {
                "--from-source" => from_source = true,
                other if other.starts_with('-') => {
                    bail!("unknown option `{other}`, try `rust help`")
                }
                other if version.is_none() => version = Some(other.to_string()),
                other => bail!("unexpected argument `{other}`, try `rust help`"),
            }
        }
        Ok(Self {
            version,
            from_source,
        })
    }
}

pub fn update(args: &[String]) -> Result<()> {
    let request = Request::parse(args)?;
    let home = cargo_home()?;
    let target = home.join("bin").join(BINARY);
    if cfg!(windows) {
        cleanup_stale_binaries(&target);
    }

    let tags = fetch_tags()?;
    let release = match &request.version {
        Some(wanted) => exact(&tags, wanted)
            .with_context(|| format!("RustScript has no release tagged {wanted}"))?,
        None => newest(&tags).context("the RustScript repository has no released version yet")?,
    };

    // An asked for version always installs, so a downgrade or a repair of a
    // broken binary both work.
    if request.version.is_none()
        && let (Some(installed), Some(latest)) = (installed_version(), release.version)
        && installed >= latest
    {
        println!(
            "rustscript is already up to date (v{})",
            format_version(installed)
        );
        return Ok(());
    }

    println!("updating rustscript from v{INSTALLED} to {}", release.tag);

    match asset_for(&release, request.from_source) {
        Some(asset) => install_asset(&release, &asset, &home, &target)?,
        None => install_from_source(&release.tag, &target)?,
    }

    println!("updated rustscript to {}", release.tag);
    warn_if_shadowed(&target);
    Ok(())
}

fn asset_for(release: &Release, from_source: bool) -> Option<Asset> {
    if from_source {
        return None;
    }
    let asset = asset::for_host(&release.tag);
    if asset.is_none() {
        eprintln!("warning: there is no prebuilt RustScript binary for {OS} {ARCH}");
        eprintln!("warning: building {} from source instead", release.tag);
    }
    asset
}

/// Download, prove the binary works, and only then touch the installed one.
fn install_asset(release: &Release, asset: &Asset, home: &Path, target: &Path) -> Result<()> {
    println!("downloading {}", asset.name);
    let archive = download(&download_url(&release.tag, &asset.name))?;
    let sums = fetch_text(&download_url(&release.tag, "SHA256SUMS"))?;
    verify_checksum(&sums, &asset.name, &archive)?;

    // Staged next to the target so the swap is a rename inside one filesystem.
    let staged = target.with_file_name(if cfg!(windows) {
        "rust.new.exe"
    } else {
        "rust.new"
    });
    extract(&archive, asset.format, &staged)?;

    if let Err(error) = verify(&staged, &release.tag) {
        discard(&staged);
        return Err(error);
    }
    swap(&staged, target)?;

    if let Err(error) = record(
        home,
        &install_key(&release.tag, &release.commit),
        asset.target,
    ) {
        eprintln!(
            "warning: rustscript is installed but cargo's install list was not updated: {error:#}"
        );
    }
    Ok(())
}

fn install_from_source(tag: &str, target: &Path) -> Result<()> {
    // Windows cannot overwrite the running binary, so cargo needs it out of
    // the way. Cargo writes its own install records on this path.
    let moved = if cfg!(windows) && target.exists() {
        Some(move_aside(target)?)
    } else {
        None
    };

    let outcome = run_cargo_install(tag).and_then(|()| {
        if target.exists() {
            Ok(())
        } else {
            bail!(
                "cargo reported success but did not install {}",
                target.display()
            )
        }
    });

    let Some(old) = moved else {
        return outcome;
    };
    match outcome {
        Ok(()) => {
            discard(&old);
            Ok(())
        }
        Err(error) => {
            if let Err(restore_error) = restore(&old, target) {
                bail!(
                    "{error:#}; restoring the previous rust binary also failed: {restore_error:#}"
                );
            }
            Err(error)
        }
    }
}

fn run_cargo_install(tag: &str) -> Result<()> {
    let status = Command::new("cargo")
        .args(["install", "--git", REPOSITORY, "--tag", tag, PACKAGE])
        .status()
        .context("could not start cargo install")?;
    if !status.success() {
        bail!("cargo install exited with {status}");
    }
    Ok(())
}

fn discard(path: &Path) {
    if let Err(error) = remove_file(path) {
        eprintln!("rust update: could not remove {}: {error}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::Request;

    #[test]
    fn a_bare_update_asks_for_the_newest_release() {
        let request = Request::parse(&[]).unwrap();
        assert_eq!(request.version, None);
        assert!(!request.from_source);
    }

    #[test]
    fn a_version_and_the_source_flag_combine_in_any_order() {
        let flag_first =
            Request::parse(&["--from-source".to_string(), "v0.2.3".to_string()]).unwrap();
        assert_eq!(flag_first.version.as_deref(), Some("v0.2.3"));
        assert!(flag_first.from_source);

        let version_first =
            Request::parse(&["v0.2.3".to_string(), "--from-source".to_string()]).unwrap();
        assert_eq!(version_first.version.as_deref(), Some("v0.2.3"));
        assert!(version_first.from_source);
    }

    #[test]
    fn unknown_options_and_extra_versions_are_rejected() {
        assert!(Request::parse(&["--latest".to_string()]).is_err());
        assert!(Request::parse(&["v0.2.3".to_string(), "v0.2.4".to_string()]).is_err());
    }
}
