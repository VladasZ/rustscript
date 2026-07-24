use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Result, anyhow, bail};

use super::iterator::{regex_captures, regex_find};
use super::methods::VArgs;
use super::native::Native;
use super::shared::{CapturesOut, MatchOut, RegexOut, captures_core, match_core, regex_core};
use super::value::{RStr, Value};

type CaptureNames = Rc<Vec<(Rc<str>, usize)>>;

#[derive(Clone)]
pub struct RegexValue {
    pub compiled: Rc<regex::Regex>,
    pattern: Rc<RStr>,
    pub names: CaptureNames,
}

#[derive(Clone)]
pub struct MatchValue {
    pub source: Rc<RStr>,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone)]
pub struct CapturesValue {
    pub source: Rc<RStr>,
    pub groups: Vec<Option<(usize, usize)>>,
    pub names: CaptureNames,
}

pub(super) fn make_regex(compiled: regex::Regex, pattern: String) -> Value {
    let names = compiled
        .capture_names()
        .enumerate()
        .filter_map(|(index, name)| name.map(|name| (Rc::from(name), index)))
        .collect();
    Native::Regex(RegexValue {
        compiled: Rc::new(compiled),
        pattern: RStr::new(pattern),
        names: Rc::new(names),
    })
    .wrap()
}

fn text_arg(args: &[Value], index: usize) -> Rc<RStr> {
    match args.get(index) {
        Some(Value::Str(text)) => text.clone(),
        Some(value) => RStr::new(value.display()),
        None => RStr::new(""),
    }
}

fn replacement_arg(args: &[Value]) -> String {
    args.get(1).map(Value::display).unwrap_or_default()
}

fn match_value(source: Rc<RStr>, start: usize, end: usize) -> Value {
    Native::RegexMatch(MatchValue { source, start, end }).wrap()
}

pub(super) fn regex_native_method(
    handle: &Rc<RefCell<Native>>,
    method: &str,
    args: &[Value],
) -> Result<Option<Value>> {
    let kind = {
        let native = handle.borrow();
        match &*native {
            Native::Regex(regex) => RegexKind::Regex(regex.clone()),
            Native::RegexMatch(found) => RegexKind::Match(found.clone()),
            Native::RegexCaptures(captures) => RegexKind::Captures(captures.clone()),
            _ => return Ok(None),
        }
    };
    let value = match kind {
        RegexKind::Regex(regex) => regex_method(&regex, method, args)?,
        RegexKind::Match(found) => match_method(&found, method)?,
        RegexKind::Captures(captures) => captures_method(&captures, method, args)?,
    };
    Ok(Some(value))
}

enum RegexKind {
    Regex(RegexValue),
    Match(MatchValue),
    Captures(CapturesValue),
}

fn regex_method(regex: &RegexValue, method: &str, args: &[Value]) -> Result<Value> {
    let source = text_arg(args, 0);
    // The lazy iterator forms cannot come out of the eager shared core.
    match method {
        "find_iter" => return Ok(regex_find(regex.clone(), source)),
        "captures_iter" => return Ok(regex_captures(regex.clone(), source)),
        _ => {}
    }
    let replacement = || replacement_arg(args);
    let Some(out) = regex_core(&regex.compiled, method, &source, &replacement) else {
        bail!("unknown method `{method}` on Regex");
    };
    Ok(match out {
        RegexOut::Bool(b) => Value::Bool(b),
        RegexOut::Text(s) => Value::str(s),
        RegexOut::Pattern => Value::Str(regex.pattern.clone()),
        RegexOut::OptSpan(span) => span
            .map(|(start, end)| Value::some(match_value(source.clone(), start, end)))
            .unwrap_or_else(Value::none),
        RegexOut::OptGroups(groups) => groups
            .map(|groups| {
                Value::some(
                    Native::RegexCaptures(CapturesValue {
                        source: source.clone(),
                        groups,
                        names: regex.names.clone(),
                    })
                    .wrap(),
                )
            })
            .unwrap_or_else(Value::none),
        RegexOut::Pieces(pieces) => Value::vec(pieces.into_iter().map(Value::str).collect()),
    })
}

fn match_method(found: &MatchValue, method: &str) -> Result<Value> {
    match match_core(method, &found.source, found.start, found.end) {
        Some(MatchOut::Text(s)) => Ok(Value::str(s)),
        Some(MatchOut::Int(i)) => Ok(Value::Int(i)),
        None => bail!("unknown method `{method}` on Match"),
    }
}

fn captures_method(captures: &CapturesValue, method: &str, args: &[Value]) -> Result<Value> {
    let names = captures.names.iter().map(|(n, i)| (n.as_ref(), *i));
    match captures_core(method, &captures.groups, names, &VArgs(args))? {
        Some(CapturesOut::Int(i)) => Ok(Value::Int(i)),
        Some(CapturesOut::OptSpan(span)) => Ok(span
            .map(|(start, end)| Value::some(match_value(captures.source.clone(), start, end)))
            .unwrap_or_else(Value::none)),
        None => bail!("unknown method `{method}` on Captures"),
    }
}

pub(super) fn capture_index(handle: &Rc<RefCell<Native>>, key: &Value) -> Result<Value> {
    let captures = {
        let native = handle.borrow();
        let Native::RegexCaptures(captures) = &*native else {
            bail!("cannot index {}", native.type_name());
        };
        captures.clone()
    };
    let index = match key {
        Value::Int(index) if *index >= 0 => *index as usize,
        Value::Str(name) => captures
            .names
            .iter()
            .find_map(|(candidate, index)| (candidate.as_ref() == name.as_str()).then_some(*index))
            .ok_or_else(|| anyhow!("no capture group named `{name}`"))?,
        _ => bail!("invalid capture index"),
    };
    let Some((start, end)) = captures.groups.get(index).copied().flatten() else {
        bail!("no match for capture group {index}");
    };
    Ok(Value::str(&captures.source[start..end]))
}
