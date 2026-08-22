//! The `Regex`, `Match` and `Captures` bridge.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use parking_lot::Mutex;

use super::bridge::VArgs;
use super::bytecode::{BuiltinId, MethodName};
use super::native::Native;
use super::shared::{CapturesOut, MatchOut, RegexOut, captures_core, match_core, regex_core};
use super::value::{RsStr, Value};

type CaptureNames = Arc<Vec<(Arc<str>, usize)>>;

#[derive(Clone)]
pub struct RegexValue {
    pub compiled: Arc<regex::Regex>,
    pattern: RsStr,
    pub names: CaptureNames,
}

#[derive(Clone)]
pub struct MatchValue {
    pub source: RsStr,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone)]
pub struct CapturesValue {
    pub source: RsStr,
    pub groups: Vec<Option<(usize, usize)>>,
    pub names: CaptureNames,
}

pub(super) fn make_regex(compiled: regex::Regex, pattern: &str) -> Value {
    let names = compiled
        .capture_names()
        .enumerate()
        .filter_map(|(index, name)| name.map(|name| (Arc::from(name), index)))
        .collect();
    Native::Regex(RegexValue {
        compiled: Arc::new(compiled),
        pattern: pattern.into(),
        names: Arc::new(names),
    })
    .wrap()
}

fn text_arg(args: &[Value], index: usize) -> RsStr {
    match args.get(index) {
        Some(Value::Str(text)) => text.clone(),
        Some(value) => value.display().into(),
        None => "".into(),
    }
}

pub(super) fn match_value(source: RsStr, start: usize, end: usize) -> Value {
    Native::RegexMatch(MatchValue { source, start, end }).wrap()
}

/// `Ok(None)` when the handle is not a regex one.
pub(super) fn regex_native_method(
    handle: &Arc<Mutex<Native>>,
    method: &MethodName,
    args: &[Value],
) -> Result<Option<Value>> {
    let kind = match &*handle.lock() {
        Native::Regex(regex) => Kind::Regex(regex.clone()),
        Native::RegexMatch(found) => Kind::Match(found.clone()),
        Native::RegexCaptures(captures) => Kind::Captures(captures.clone()),
        _ => return Ok(None),
    };
    Ok(Some(match kind {
        Kind::Regex(regex) => regex_method(&regex, method, args)?,
        Kind::Match(found) => match_method(&found, method)?,
        Kind::Captures(captures) => captures_method(&captures, method, args)?,
    }))
}

enum Kind {
    Regex(RegexValue),
    Match(MatchValue),
    Captures(CapturesValue),
}

fn regex_method(regex: &RegexValue, method: &MethodName, args: &[Value]) -> Result<Value> {
    let source = text_arg(args, 0);
    // The iterator forms are lazy, so they stay out of the shared core.
    match method.id {
        BuiltinId::FindIter => return Ok(super::iterator::regex_find(regex.clone(), source)),
        BuiltinId::CapturesIter => {
            return Ok(super::iterator::regex_captures(regex.clone(), source));
        }
        _ => {}
    }
    let replacement = || args.get(1).map(Value::display).unwrap_or_default();
    let Some(out) = regex_core(&regex.compiled, method.id, &source, &replacement) else {
        bail!("unknown method `{}` on Regex", method.text);
    };
    Ok(match out {
        RegexOut::Bool(b) => Value::Bool(b),
        RegexOut::Text(s) => Value::str(s),
        RegexOut::Pattern => Value::Str(regex.pattern.clone()),
        RegexOut::OptSpan(span) => span.map_or_else(Value::none, |(start, end)| {
            Value::some(match_value(source.clone(), start, end))
        }),
        RegexOut::OptGroups(groups) => groups.map_or_else(Value::none, |groups| {
            Value::some(
                Native::RegexCaptures(CapturesValue {
                    source: source.clone(),
                    groups,
                    names: regex.names.clone(),
                })
                .wrap(),
            )
        }),
        RegexOut::Pieces(pieces) => Value::vec(pieces.into_iter().map(Value::str).collect()),
    })
}

fn match_method(found: &MatchValue, method: &MethodName) -> Result<Value> {
    match match_core(method.id, &found.source, found.start, found.end) {
        Some(MatchOut::Text(s)) => Ok(Value::str(s)),
        Some(MatchOut::Int(i)) => Ok(Value::Int(i)),
        None => bail!("unknown method `{}` on Match", method.text),
    }
}

fn captures_method(captures: &CapturesValue, method: &MethodName, args: &[Value]) -> Result<Value> {
    let names = captures.names.iter().map(|(n, i)| (n.as_ref(), *i));
    match captures_core(method.id, &captures.groups, names, &VArgs(args))? {
        Some(CapturesOut::Int(i)) => Ok(Value::Int(i)),
        Some(CapturesOut::OptSpan(span)) => Ok(span.map_or_else(Value::none, |(start, end)| {
            Value::some(match_value(captures.source.clone(), start, end))
        })),
        None => bail!("unknown method `{}` on Captures", method.text),
    }
}

fn group_by_name(captures: &CapturesValue, name: &str) -> Option<usize> {
    captures
        .names
        .iter()
        .find_map(|(candidate, index)| (candidate.as_ref() == name).then_some(*index))
}

/// `caps[1]` and `caps["name"]`, which panic on a missing group.
pub(super) fn capture_index(handle: &Arc<Mutex<Native>>, key: &Value) -> Result<Value> {
    let captures = {
        let native = handle.lock();
        let Native::RegexCaptures(captures) = &*native else {
            bail!("cannot index {}", native.type_name());
        };
        captures.clone()
    };
    let index = match key {
        Value::Int(index) if *index >= 0 => usize::try_from(*index)?,
        Value::Str(name) => group_by_name(&captures, name)
            .ok_or_else(|| anyhow!("no capture group named `{name}`"))?,
        _ => bail!("invalid capture index"),
    };
    let Some((start, end)) = captures.groups.get(index).copied().flatten() else {
        bail!("no match for capture group {index}");
    };
    Ok(Value::str(&captures.source[start..end]))
}
