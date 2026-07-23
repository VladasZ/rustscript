use std::collections::HashMap;
use std::process::Command;

use anyhow::{Context, Result, bail};

pub const REPOSITORY: &str = "https://github.com/VladasZ/rustscript";
pub const PACKAGE: &str = "run-rs";
pub const INSTALLED: &str = env!("CARGO_PKG_VERSION");

pub type Version = (u64, u64, u64);

pub struct Release {
    pub tag: String,
    pub commit: String,
    /// `None` for a prerelease, which is only ever reached by asking for it.
    pub version: Option<Version>,
}

pub fn installed_version() -> Option<Version> {
    parse_version(INSTALLED)
}

pub fn format_version((major, minor, patch): Version) -> String {
    format!("{major}.{minor}.{patch}")
}

pub fn parse_version(tag: &str) -> Option<Version> {
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

pub fn fetch_tags() -> Result<String> {
    let output = Command::new("git")
        .args(["ls-remote", "--tags", REPOSITORY])
        .output()
        .context("could not query the RustScript repository")?;
    if !output.status.success() {
        bail!("git ls-remote exited with {}", output.status);
    }
    String::from_utf8(output.stdout).context("git returned non-UTF-8 tags")
}

/// Every tag mapped to the commit it resolves to, which is what cargo records.
fn tag_commits(ls_remote: &str) -> HashMap<String, String> {
    let mut tags = HashMap::new();
    for line in ls_remote.lines() {
        let mut parts = line.split_whitespace();
        let (Some(commit), Some(reference)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Some(tag) = reference.strip_prefix("refs/tags/") else {
            continue;
        };
        // An annotated tag lists the tag object under its own name and the
        // commit it points at as a peeled entry, so the peeled one wins.
        if let Some(name) = tag.strip_suffix("^{}") {
            tags.insert(name.to_string(), commit.to_string());
        } else {
            tags.entry(tag.to_string())
                .or_insert_with(|| commit.to_string());
        }
    }
    tags
}

pub fn newest(ls_remote: &str) -> Option<Release> {
    tag_commits(ls_remote)
        .into_iter()
        .filter_map(|(tag, commit)| {
            Some(Release {
                version: Some(parse_version(&tag)?),
                tag,
                commit,
            })
        })
        .max_by_key(|release| release.version)
}

/// The release for an exact tag, with or without the `v` the user typed.
pub fn exact(ls_remote: &str, wanted: &str) -> Option<Release> {
    let tags = tag_commits(ls_remote);
    let prefixed = format!("v{wanted}");
    for name in [wanted, prefixed.as_str()] {
        if let Some(commit) = tags.get(name) {
            return Some(Release {
                tag: name.to_string(),
                commit: commit.clone(),
                version: parse_version(name),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::{exact, newest, parse_version};

    const LS_REMOTE: &str = r"1111111111111111111111111111111111111111	refs/tags/v0.1.0
2222222222222222222222222222222222222222	refs/tags/v0.2.0
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa	refs/tags/v0.2.0^{}
3333333333333333333333333333333333333333	refs/tags/v0.2
4444444444444444444444444444444444444444	refs/tags/v0.3.0-rc.1
5555555555555555555555555555555555555555	refs/tags/v0.10.0
";

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
        let newest = newest(LS_REMOTE).unwrap();
        assert_eq!(newest.tag, "v0.10.0");
        assert_eq!(newest.version, Some((0, 10, 0)));
        assert_eq!(newest.commit, "5555555555555555555555555555555555555555");
    }

    #[test]
    fn a_repository_without_releases_has_no_newest_release() {
        let ls_remote = "3333333333333333333333333333333333333333	refs/tags/v0.2\n";
        assert!(newest(ls_remote).is_none());
        assert!(newest("").is_none());
    }

    #[test]
    fn an_annotated_tag_resolves_to_the_commit_it_points_at() {
        let release = exact(LS_REMOTE, "v0.2.0").unwrap();
        assert_eq!(release.commit, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    }

    #[test]
    fn an_exact_tag_is_found_with_or_without_the_v() {
        assert_eq!(exact(LS_REMOTE, "0.1.0").unwrap().tag, "v0.1.0");
        assert_eq!(exact(LS_REMOTE, "v0.1.0").unwrap().tag, "v0.1.0");
        assert!(exact(LS_REMOTE, "v9.9.9").is_none());
    }

    #[test]
    fn a_prerelease_is_reachable_by_asking_for_it() {
        let release = exact(LS_REMOTE, "v0.3.0-rc.1").unwrap();
        assert_eq!(release.tag, "v0.3.0-rc.1");
        assert_eq!(release.version, None);
    }
}
