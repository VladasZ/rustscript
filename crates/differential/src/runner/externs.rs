//! The bridged crates a case may name. Cargo builds them once as the dependencies of the
//! examples crate, and every case links the rlibs with `--extern`. So a case that uses
//! `serde_json` compiles with one `rustc` run and no cargo project of its own.

use std::collections::BTreeMap;
use std::env::var_os;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// The crates a generated program may use.
pub const EXTERN_CRATES: [&str; 6] = ["serde", "serde_json", "toml", "regex", "chrono", "tokio"];

const EXAMPLES_PACKAGE: &str = "rustscript-examples";

/// Any example does, cargo builds every dependency of the package for it.
const ANY_EXAMPLE: &str = "json_typed";

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    resolve: Resolve,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    id: String,
}

#[derive(Deserialize)]
struct Resolve {
    nodes: Vec<ResolveNode>,
}

#[derive(Deserialize)]
struct ResolveNode {
    id: String,
    deps: Vec<ResolveDep>,
}

#[derive(Deserialize)]
struct ResolveDep {
    /// the name the code uses, `serde_json`
    name: String,
    pkg: String,
}

/// One line of `cargo build --message-format=json`.
#[derive(Deserialize)]
struct BuildMessage {
    reason: String,
    #[serde(default)]
    package_id: String,
    #[serde(default)]
    filenames: Vec<PathBuf>,
}

/// The `rustc` arguments that make the crates of `EXTERN_CRATES` nameable in a case.
pub fn build(workspace: &Path) -> Result<Vec<OsString>> {
    let cargo = var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let wanted = wanted_packages(workspace, &cargo)?;
    let output = Command::new(&cargo)
        .args(["build", "--release", "-p", EXAMPLES_PACKAGE, "--example"])
        .args([ANY_EXAMPLE, "--message-format=json"])
        .current_dir(workspace)
        .output()
        .context("failed to build the bridged crates")?;
    if !output.status.success() {
        bail!(
            "the build of the bridged crates failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mut rlibs: BTreeMap<&str, PathBuf> = BTreeMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        // cargo also prints lines of other shapes, a line that is no artifact is skipped
        let Ok(message) = serde_json::from_str::<BuildMessage>(line) else {
            continue;
        };
        if message.reason != "compiler-artifact" {
            continue;
        }
        let Some((name, _)) = wanted.iter().find(|(_, id)| **id == message.package_id) else {
            continue;
        };
        if let Some(rlib) = message
            .filenames
            .into_iter()
            .find(|file| file.extension().is_some_and(|ext| ext == "rlib"))
        {
            rlibs.insert(name, rlib);
        }
    }
    let mut args = Vec::new();
    let mut deps_dir = None;
    for name in EXTERN_CRATES {
        let Some(rlib) = rlibs.get(name) else {
            bail!("cargo built no rlib for `{name}`");
        };
        deps_dir = rlib.parent().map(Path::to_path_buf);
        let mut spec = OsString::from(format!("{name}="));
        spec.push(rlib);
        args.push(OsString::from("--extern"));
        args.push(spec);
    }
    // the rlibs name their own dependencies, the proc macro behind `serde::Deserialize` too
    if let Some(dir) = deps_dir {
        let mut search = OsString::from("dependency=");
        search.push(dir);
        args.push(OsString::from("-L"));
        args.push(search);
    }
    Ok(args)
}

/// The package id behind every name of `EXTERN_CRATES`, as the examples crate depends on it. A
/// workspace can hold 2 versions of a crate, the name alone does not say which one.
fn wanted_packages(workspace: &Path, cargo: &OsString) -> Result<Vec<(&'static str, String)>> {
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1"])
        .current_dir(workspace)
        .output()
        .context("failed to run cargo metadata")?;
    if !output.status.success() {
        bail!(
            "cargo metadata failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let metadata: Metadata =
        serde_json::from_slice(&output.stdout).context("parse the cargo metadata")?;
    let examples = metadata
        .packages
        .iter()
        .find(|package| package.name == EXAMPLES_PACKAGE)
        .with_context(|| format!("no package `{EXAMPLES_PACKAGE}` in the workspace"))?;
    let node = metadata
        .resolve
        .nodes
        .iter()
        .find(|node| node.id == examples.id)
        .context("the examples crate is not in the dependency graph")?;
    EXTERN_CRATES
        .iter()
        .map(|name| {
            let dep = node
                .deps
                .iter()
                .find(|dep| dep.name == *name)
                .with_context(|| format!("the examples crate does not depend on `{name}`"))?;
            Ok((*name, dep.pkg.clone()))
        })
        .collect()
}
