//! The `Regex`, `Match` and `Captures` bridge for the parallel engine. Mirrors
//! `regex_bridge.rs` on the `Send + Sync` value model, so a `#[tokio::main]`
//! script can compile a pattern once and match from concurrent tasks.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use parking_lot::Mutex;

use super::pbridge::PArgs;
use super::pnative::PNative;
use super::pvalue::PValue;
use super::shared::{CapturesOut, MatchOut, RegexOut, captures_core, match_core, regex_core};

type CaptureNames = Arc<Vec<(Arc<str>, usize)>>;

#[derive(Clone)]
pub struct PRegexValue {
    pub compiled: Arc<regex::Regex>,
    pattern: Arc<str>,
    pub names: CaptureNames,
}

#[derive(Clone)]
pub struct PMatchValue {
    pub source: Arc<str>,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone)]
pub struct PCapturesValue {
    pub source: Arc<str>,
    pub groups: Vec<Option<(usize, usize)>>,
    pub names: CaptureNames,
}

pub(super) fn make_regex(compiled: regex::Regex, pattern: &str) -> PValue {
    let names = compiled
        .capture_names()
        .enumerate()
        .filter_map(|(index, name)| name.map(|name| (Arc::from(name), index)))
        .collect();
    PNative::Regex(PRegexValue {
        compiled: Arc::new(compiled),
        pattern: Arc::from(pattern),
        names: Arc::new(names),
    })
    .wrap()
}

fn text_arg(args: &[PValue], index: usize) -> Arc<str> {
    match args.get(index) {
        Some(PValue::Str(text)) => text.clone(),
        Some(value) => Arc::from(value.display().as_str()),
        None => Arc::from(""),
    }
}

fn match_value(source: Arc<str>, start: usize, end: usize) -> PValue {
    PNative::RegexMatch(PMatchValue { source, start, end }).wrap()
}

fn captures_value(regex: &PRegexValue, source: Arc<str>, captures: &regex::Captures) -> PValue {
    let groups = (0..captures.len())
        .map(|index| {
            captures
                .get(index)
                .map(|found| (found.start(), found.end()))
        })
        .collect();
    PNative::RegexCaptures(PCapturesValue {
        source,
        groups,
        names: regex.names.clone(),
    })
    .wrap()
}

/// Dispatch a method on a regex-family handle. `Ok(None)` when the handle is
/// not one of these, so the caller can keep looking.
pub(super) fn regex_native_method(
    handle: &Arc<Mutex<PNative>>,
    method: &str,
    args: &[PValue],
) -> Result<Option<PValue>> {
    let kind = match &*handle.lock() {
        PNative::Regex(regex) => Kind::Regex(regex.clone()),
        PNative::RegexMatch(found) => Kind::Match(found.clone()),
        PNative::RegexCaptures(captures) => Kind::Captures(captures.clone()),
        _ => return Ok(None),
    };
    Ok(Some(match kind {
        Kind::Regex(regex) => regex_method(&regex, method, args)?,
        Kind::Match(found) => match_method(&found, method)?,
        Kind::Captures(captures) => captures_method(&captures, method, args)?,
    }))
}

enum Kind {
    Regex(PRegexValue),
    Match(PMatchValue),
    Captures(PCapturesValue),
}

fn regex_method(regex: &PRegexValue, method: &str, args: &[PValue]) -> Result<PValue> {
    let source = text_arg(args, 0);
    // The iterator forms are eager here, unlike the fast engine's lazy ones,
    // so they stay out of the shared core on both sides.
    match method {
        "find_iter" => {
            return Ok(PValue::vec(
                regex
                    .compiled
                    .find_iter(&source)
                    .map(|found| match_value(source.clone(), found.start(), found.end()))
                    .collect(),
            ));
        }
        "captures_iter" => {
            return Ok(PValue::vec(
                regex
                    .compiled
                    .captures_iter(&source)
                    .map(|captures| captures_value(regex, source.clone(), &captures))
                    .collect(),
            ));
        }
        _ => {}
    }
    let replacement = || args.get(1).map(PValue::display).unwrap_or_default();
    let Some(out) = regex_core(&regex.compiled, method, &source, &replacement) else {
        bail!("method `{method}` on Regex is not supported in tokio mode");
    };
    Ok(match out {
        RegexOut::Bool(b) => PValue::Bool(b),
        RegexOut::Text(s) => PValue::str(s),
        RegexOut::Pattern => PValue::Str(regex.pattern.clone()),
        RegexOut::OptSpan(span) => span.map_or_else(PValue::none, |(start, end)| {
            PValue::some(match_value(source.clone(), start, end))
        }),
        RegexOut::OptGroups(groups) => groups.map_or_else(PValue::none, |groups| {
            PValue::some(
                PNative::RegexCaptures(PCapturesValue {
                    source: source.clone(),
                    groups,
                    names: regex.names.clone(),
                })
                .wrap(),
            )
        }),
        RegexOut::Pieces(pieces) => PValue::vec(pieces.into_iter().map(PValue::str).collect()),
    })
}

fn match_method(found: &PMatchValue, method: &str) -> Result<PValue> {
    match match_core(method, &found.source, found.start, found.end) {
        Some(MatchOut::Text(s)) => Ok(PValue::str(s)),
        Some(MatchOut::Int(i)) => Ok(PValue::Int(i)),
        None => bail!("method `{method}` on Match is not supported in tokio mode"),
    }
}

fn captures_method(captures: &PCapturesValue, method: &str, args: &[PValue]) -> Result<PValue> {
    let names = captures.names.iter().map(|(n, i)| (n.as_ref(), *i));
    match captures_core(method, &captures.groups, names, &PArgs(args))? {
        Some(CapturesOut::Int(i)) => Ok(PValue::Int(i)),
        Some(CapturesOut::OptSpan(span)) => Ok(span.map_or_else(PValue::none, |(start, end)| {
            PValue::some(match_value(captures.source.clone(), start, end))
        })),
        None => bail!("method `{method}` on Captures is not supported in tokio mode"),
    }
}

fn group_by_name(captures: &PCapturesValue, name: &str) -> Option<usize> {
    captures
        .names
        .iter()
        .find_map(|(candidate, index)| (candidate.as_ref() == name).then_some(*index))
}

/// `caps[1]` and `caps["name"]`, which panic in real Rust on a missing group.
pub(super) fn capture_index(handle: &Arc<Mutex<PNative>>, key: &PValue) -> Result<PValue> {
    let captures = {
        let native = handle.lock();
        let PNative::RegexCaptures(captures) = &*native else {
            bail!("cannot index {}", native.type_name());
        };
        captures.clone()
    };
    let index = match key {
        PValue::Int(index) if *index >= 0 => *index as usize,
        PValue::Str(name) => group_by_name(&captures, name)
            .ok_or_else(|| anyhow!("no capture group named `{name}`"))?,
        _ => bail!("invalid capture index"),
    };
    let Some((start, end)) = captures.groups.get(index).copied().flatten() else {
        bail!("no match for capture group {index}");
    };
    Ok(PValue::str(&captures.source[start..end]))
}
