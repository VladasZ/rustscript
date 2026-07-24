use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::runner::RunResult;

/// Known divergences the campaign reports but does not fail on. Each entry is
/// an open interpreter bug with a visible paper trail. Fix the bug, delete
/// the entry. A finding that matches no entry still fails the run, so the
/// generator never has to shrink its grammar around a known gap.
#[derive(Debug, Default, Deserialize)]
pub struct Quarantine {
    #[serde(default)]
    pub known: Vec<KnownDivergence>,
}

#[derive(Debug, Deserialize)]
pub struct KnownDivergence {
    /// The `Classification` debug name, for example `SemanticMismatch`.
    pub classification: String,
    /// The digit-normalized signature as `RunResult::signature` produces it.
    /// A `*` matches any substring, so one root cause that surfaces with
    /// varying payloads needs one entry, not one per payload pair.
    pub signature: String,
    pub date: String,
    pub note: String,
}

impl Quarantine {
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join("crates/differential/quarantine.toml");
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn matches(&self, result: &RunResult) -> Option<&KnownDivergence> {
        let classification = format!("{:?}", result.classification);
        let signature = result.signature();
        self.known.iter().find(|entry| {
            entry.classification == classification && wildcard_match(&entry.signature, &signature)
        })
    }
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == text;
    }
    let segments: Vec<&str> = pattern.split('*').collect();
    let first = segments[0];
    let Some(mut remaining) = text.strip_prefix(first) else {
        return false;
    };
    let last = segments[segments.len() - 1];
    for segment in &segments[1..segments.len() - 1] {
        if segment.is_empty() {
            continue;
        }
        let Some(found) = remaining.find(segment) else {
            return false;
        };
        remaining = &remaining[found + segment.len()..];
    }
    remaining.ends_with(last)
}

#[cfg(test)]
mod tests {
    use super::wildcard_match;

    #[test]
    fn wildcards_match_signatures() {
        assert!(wildcard_match("exact", "exact"));
        assert!(!wildcard_match("exact", "exactly"));
        assert!(wildcard_match(
            "attempt to * with overflow",
            "attempt to add with overflow"
        ));
        assert!(wildcard_match(
            "attempt to * <> *",
            "attempt to negate with overflow <> attempt to multiply with overflow"
        ));
        assert!(!wildcard_match("attempt to * <> *", "index out of bounds"));
        assert!(wildcard_match("*overflow", "attempt to add with overflow"));
        assert!(wildcard_match(
            "rust error:*",
            "rust error: number too large to fit in target type"
        ));
        assert!(!wildcard_match("a*b*c", "acb"));
    }
}
