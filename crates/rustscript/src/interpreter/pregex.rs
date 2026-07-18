//! The `Regex`, `Match` and `Captures` bridge for the parallel engine. Mirrors
//! `regex_bridge.rs` on the `Send + Sync` value model, so a `#[tokio::main]`
//! script can compile a pattern once and match from concurrent tasks.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use parking_lot::Mutex;

use super::pnative::PNative;
use super::pvalue::PValue;

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
    let replacement = || args.get(1).map(PValue::display).unwrap_or_default();
    Ok(match method {
        "is_match" => PValue::Bool(regex.compiled.is_match(&source)),
        "find" => regex
            .compiled
            .find(&source)
            .map_or_else(PValue::none, |found| {
                PValue::some(match_value(source.clone(), found.start(), found.end()))
            }),
        "captures" => regex
            .compiled
            .captures(&source)
            .map_or_else(PValue::none, |captures| {
                PValue::some(captures_value(regex, source.clone(), &captures))
            }),
        "find_iter" => PValue::vec(
            regex
                .compiled
                .find_iter(&source)
                .map(|found| match_value(source.clone(), found.start(), found.end()))
                .collect(),
        ),
        "captures_iter" => PValue::vec(
            regex
                .compiled
                .captures_iter(&source)
                .map(|captures| captures_value(regex, source.clone(), &captures))
                .collect(),
        ),
        "replace" => PValue::str(
            regex
                .compiled
                .replacen(&source, 1, replacement().as_str())
                .into_owned(),
        ),
        "replace_all" => PValue::str(
            regex
                .compiled
                .replace_all(&source, replacement().as_str())
                .into_owned(),
        ),
        "split" => PValue::vec(regex.compiled.split(&source).map(PValue::str).collect()),
        "as_str" => PValue::Str(regex.pattern.clone()),
        _ => bail!("method `{method}` on Regex is not supported in tokio mode"),
    })
}

fn match_method(found: &PMatchValue, method: &str) -> Result<PValue> {
    Ok(match method {
        "as_str" => PValue::str(&found.source[found.start..found.end]),
        "start" => PValue::Int(found.start as i64),
        "end" => PValue::Int(found.end as i64),
        _ => bail!("method `{method}` on Match is not supported in tokio mode"),
    })
}

fn captures_method(captures: &PCapturesValue, method: &str, args: &[PValue]) -> Result<PValue> {
    Ok(match method {
        "get" => {
            let index = match args.first() {
                Some(PValue::Int(index)) if *index >= 0 => *index as usize,
                _ => bail!("captures get needs a non-negative index"),
            };
            capture_group(captures, index)
        }
        "name" => {
            let name = args.first().map(PValue::display).unwrap_or_default();
            group_by_name(captures, &name)
                .map_or_else(PValue::none, |index| capture_group(captures, index))
        }
        "len" => PValue::Int(captures.groups.len() as i64),
        _ => bail!("method `{method}` on Captures is not supported in tokio mode"),
    })
}

fn group_by_name(captures: &PCapturesValue, name: &str) -> Option<usize> {
    captures
        .names
        .iter()
        .find_map(|(candidate, index)| (candidate.as_ref() == name).then_some(*index))
}

fn capture_group(captures: &PCapturesValue, index: usize) -> PValue {
    captures
        .groups
        .get(index)
        .copied()
        .flatten()
        .map_or_else(PValue::none, |(start, end)| {
            PValue::some(match_value(captures.source.clone(), start, end))
        })
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
