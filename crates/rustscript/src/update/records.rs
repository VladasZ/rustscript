use std::collections::BTreeMap;
use std::fs::{read_to_string, rename, write};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, from_str, from_value, to_string, to_value};

use super::install::BINARY;
use super::release::{PACKAGE, REPOSITORY};

/// `.crates.toml`, the list `cargo install --list` prints.
#[derive(Deserialize, Serialize, Default)]
struct CratesToml {
    #[serde(default)]
    v1: BTreeMap<String, Vec<String>>,
}

/// `.crates2.json`, the record `cargo install` and `cargo install-update` read. The entry of any
/// other package is carried over as it is, whatever fields its cargo wrote.
#[derive(Deserialize, Serialize)]
struct Crates2Json {
    #[serde(default)]
    installs: BTreeMap<String, Value>,
    v: u32,
}

/// One install entry, the fields cargo writes for its own installs.
#[derive(Deserialize, Serialize)]
struct Install {
    version_req: Option<String>,
    bins: Vec<String>,
    features: Vec<String>,
    all_features: bool,
    no_default_features: bool,
    profile: String,
    target: String,
    rustc: String,
}

/// The one field of a stale entry worth carrying over.
#[derive(Deserialize, Default)]
struct PreviousInstall {
    #[serde(default)]
    rustc: String,
}

/// The `name version (source)` key cargo tracks an install under. A download
/// is recorded as the git source of its tag so `cargo install-update` stays
/// accurate.
pub fn install_key(tag: &str, commit: &str) -> String {
    let version = tag.strip_prefix('v').unwrap_or(tag);
    format!("{PACKAGE} {version} (git+{REPOSITORY}?tag={tag}#{commit})")
}

/// Without these cargo believes the old version is still installed.
pub fn record(cargo_home: &Path, key: &str, fallback_target: &str) -> Result<()> {
    let info = rustc_info();
    let (rustc, target) = match &info {
        Some((rustc, host)) => (Some(rustc.as_str()), host.as_str()),
        None => (None, fallback_target),
    };
    patch_crates_toml(&cargo_home.join(".crates.toml"), key)?;
    patch_crates2_json(&cargo_home.join(".crates2.json"), key, target, rustc)
}

fn package_of(key: &str) -> &str {
    key.split_whitespace().next().unwrap_or_default()
}

fn is_stale(key: &str) -> bool {
    package_of(key) == PACKAGE
}

/// Both files are cargo's record of every installed tool, so neither is ever left half written.
fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let staged = path.with_extension(format!("tmp.{}", std::process::id()));
    write(&staged, text).with_context(|| format!("could not write {}", staged.display()))?;
    rename(&staged, path).with_context(|| format!("could not replace {}", path.display()))
}

fn patch_crates_toml(path: &Path, key: &str) -> Result<()> {
    let mut doc: CratesToml = if path.exists() {
        let text =
            read_to_string(path).with_context(|| format!("could not read {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("could not parse {}", path.display()))?
    } else {
        CratesToml::default()
    };

    doc.v1.retain(|entry, _| !is_stale(entry));
    doc.v1.insert(key.to_string(), vec![BINARY.to_string()]);

    let text = toml::to_string(&doc).context("could not serialize the cargo install list")?;
    write_atomic(path, &text)
}

fn patch_crates2_json(path: &Path, key: &str, target: &str, rustc: Option<&str>) -> Result<()> {
    let mut doc: Crates2Json = if path.exists() {
        let text =
            read_to_string(path).with_context(|| format!("could not read {}", path.display()))?;
        from_str(&text).with_context(|| format!("could not parse {}", path.display()))?
    } else {
        Crates2Json {
            installs: BTreeMap::new(),
            v: 1,
        }
    };
    doc.v = 1;

    let stale: Vec<String> = doc
        .installs
        .keys()
        .filter(|k| is_stale(k))
        .cloned()
        .collect();
    let mut previous_rustc = String::new();
    for entry in stale {
        if let Some(value) = doc.installs.remove(&entry) {
            let previous: PreviousInstall = from_value(value).with_context(|| {
                format!(
                    "the entry of {entry} in {} is not an install record",
                    path.display()
                )
            })?;
            previous_rustc = previous.rustc;
        }
    }

    let install = Install {
        version_req: None,
        bins: vec![BINARY.to_string()],
        features: Vec::new(),
        all_features: false,
        no_default_features: false,
        profile: "release".to_string(),
        target: target.to_string(),
        rustc: rustc.map_or(previous_rustc, str::to_string),
    };
    doc.installs.insert(
        key.to_string(),
        to_value(install).context("could not serialize the install record")?,
    );

    let text = to_string(&doc).context("could not serialize the cargo install list")?;
    write_atomic(path, &text)
}

/// The building toolchain is unknowable, so the local one is recorded. Its
/// host triple is the honest `target` too.
fn rustc_info() -> Option<(String, String)> {
    let output = Command::new("rustc").arg("-vV").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let host = text
        .lines()
        .find_map(|line| line.strip_prefix("host: "))?
        .to_string();
    Some((text, host))
}

#[cfg(test)]
mod tests {
    use std::fs::{read_dir, read_to_string, write};
    use std::path::Path;

    use pretty_assertions::assert_eq;
    use serde_json::{from_str, from_value};
    use tempfile::tempdir;

    use super::{
        BINARY, Crates2Json, CratesToml, Install, install_key, patch_crates_toml,
        patch_crates2_json,
    };

    const KEY: &str = "run-rs 0.2.7 (git+https://github.com/VladasZ/rustscript?tag=v0.2.7#abc123)";
    const RIPGREP: &str = "ripgrep 14.1.1 (registry+https://github.com/rust-lang/crates.io-index)";

    fn read_toml(path: &Path) -> CratesToml {
        toml::from_str(&read_to_string(path).unwrap()).unwrap()
    }

    fn read_json(path: &Path) -> Crates2Json {
        from_str(&read_to_string(path).unwrap()).unwrap()
    }

    fn our_entry(doc: &Crates2Json) -> Install {
        from_value(doc.installs[KEY].clone()).unwrap()
    }

    /// The staged file must be gone once the real one is in place.
    fn only_file(dir: &Path, name: &str) {
        let names: Vec<String> = read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![name.to_string()]);
    }

    #[test]
    fn the_install_key_matches_what_a_source_install_writes() {
        assert_eq!(
            install_key("v0.2.6", "051dc69fe14e005b6e768ac1e63afbbb2e9dd8e2"),
            "run-rs 0.2.6 (git+https://github.com/VladasZ/rustscript?tag=v0.2.6#051dc69fe14e005b6e768ac1e63afbbb2e9dd8e2)"
        );
    }

    #[test]
    fn the_old_entry_is_replaced_and_other_crates_are_left_alone() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".crates.toml");
        write(
            &path,
            r#"[v1]
"ripgrep 14.1.1 (registry+https://github.com/rust-lang/crates.io-index)" = ["rg"]
"run-rs 0.2.6 (git+https://github.com/VladasZ/rustscript?tag=v0.2.6#051dc69)" = ["rust"]
"#,
        )
        .unwrap();

        patch_crates_toml(&path, KEY).unwrap();

        let doc = read_toml(&path);
        assert_eq!(doc.v1.len(), 2);
        assert_eq!(doc.v1[KEY], vec![BINARY.to_string()]);
        assert_eq!(doc.v1[RIPGREP], vec!["rg".to_string()]);
        only_file(dir.path(), ".crates.toml");
    }

    #[test]
    fn a_missing_crates_toml_is_created() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".crates.toml");

        patch_crates_toml(&path, KEY).unwrap();

        assert!(read_toml(&path).v1.contains_key(KEY));
    }

    /// The ripgrep entry has fields this code never wrote, and keeps them byte for byte.
    #[test]
    fn the_json_entry_is_replaced_and_the_new_toolchain_recorded() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".crates2.json");
        write(
            &path,
            r#"{"installs":{
"ripgrep 14.1.1 (registry+https://github.com/rust-lang/crates.io-index)":{"bins":["rg"],"future_field":7},
"run-rs 0.2.6 (git+https://github.com/VladasZ/rustscript?tag=v0.2.6#051dc69)":{"bins":["rust"],"rustc":"rustc 1.90.0","target":"aarch64-apple-darwin"}},"v":1}"#,
        )
        .unwrap();

        patch_crates2_json(&path, KEY, "aarch64-apple-darwin", Some("rustc 1.96.1")).unwrap();

        let doc = read_json(&path);
        assert_eq!(doc.installs.len(), 2);
        let ours = our_entry(&doc);
        assert_eq!(ours.rustc, "rustc 1.96.1");
        assert_eq!(ours.target, "aarch64-apple-darwin");
        assert_eq!(ours.bins, vec![BINARY.to_string()]);
        assert_eq!(ours.profile, "release");
        assert_eq!(
            doc.installs[RIPGREP].to_string(),
            r#"{"bins":["rg"],"future_field":7}"#
        );
        only_file(dir.path(), ".crates2.json");
    }

    #[test]
    fn an_unreadable_toolchain_keeps_the_previous_one() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".crates2.json");
        write(
            &path,
            r#"{"installs":{"run-rs 0.2.6 (git+x#1)":{"bins":["rust"],"rustc":"rustc 1.90.0"}},"v":1}"#,
        )
        .unwrap();

        patch_crates2_json(&path, KEY, "x86_64-unknown-linux-musl", None).unwrap();

        assert_eq!(our_entry(&read_json(&path)).rustc, "rustc 1.90.0");
    }

    #[test]
    fn a_missing_crates2_json_is_created() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".crates2.json");

        patch_crates2_json(
            &path,
            KEY,
            "x86_64-unknown-linux-musl",
            Some("rustc 1.96.1"),
        )
        .unwrap();

        let doc = read_json(&path);
        assert_eq!(doc.v, 1);
        assert_eq!(our_entry(&doc).target, "x86_64-unknown-linux-musl");
    }
}
