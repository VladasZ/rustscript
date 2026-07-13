use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Result, anyhow, bail};

use super::iterator::{regex_captures, regex_find};
use super::native::Native;
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

fn captures_value(regex: &RegexValue, source: Rc<RStr>, captures: &regex::Captures) -> Value {
    let groups = (0..captures.len())
        .map(|index| {
            captures
                .get(index)
                .map(|found| (found.start(), found.end()))
        })
        .collect();
    Native::RegexCaptures(CapturesValue {
        source,
        groups,
        names: regex.names.clone(),
    })
    .wrap()
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
    Ok(match method {
        "is_match" => Value::Bool(regex.compiled.is_match(&source)),
        "find" => regex
            .compiled
            .find(&source)
            .map(|found| match_value(source.clone(), found.start(), found.end()))
            .map(Value::some)
            .unwrap_or_else(Value::none),
        "find_iter" => regex_find(regex.clone(), source),
        "captures" => regex
            .compiled
            .captures(&source)
            .map(|captures| captures_value(regex, source.clone(), &captures))
            .map(Value::some)
            .unwrap_or_else(Value::none),
        "captures_iter" => regex_captures(regex.clone(), source),
        "replace" => Value::str(
            regex
                .compiled
                .replacen(&source, 1, replacement_arg(args).as_str())
                .into_owned(),
        ),
        "replace_all" => Value::str(
            regex
                .compiled
                .replace_all(&source, replacement_arg(args).as_str())
                .into_owned(),
        ),
        "split" => Value::vec(regex.compiled.split(&source).map(Value::str).collect()),
        "as_str" => Value::Str(regex.pattern.clone()),
        _ => bail!("unknown method `{method}` on Regex"),
    })
}

fn match_method(found: &MatchValue, method: &str) -> Result<Value> {
    Ok(match method {
        "as_str" => Value::str(&found.source[found.start..found.end]),
        "start" => Value::Int(found.start as i64),
        "end" => Value::Int(found.end as i64),
        _ => bail!("unknown method `{method}` on Match"),
    })
}

fn captures_method(captures: &CapturesValue, method: &str, args: &[Value]) -> Result<Value> {
    Ok(match method {
        "get" => {
            let index = match args.first() {
                Some(Value::Int(index)) if *index >= 0 => *index as usize,
                _ => bail!("captures get needs a non-negative index"),
            };
            capture_group(captures, index)
        }
        "name" => {
            let name = args.first().map(Value::display).unwrap_or_default();
            captures
                .names
                .iter()
                .find_map(|(candidate, index)| (candidate.as_ref() == name).then_some(*index))
                .map(|index| capture_group(captures, index))
                .unwrap_or_else(Value::none)
        }
        "len" => Value::Int(captures.groups.len() as i64),
        _ => bail!("unknown method `{method}` on Captures"),
    })
}

fn capture_group(captures: &CapturesValue, index: usize) -> Value {
    captures
        .groups
        .get(index)
        .copied()
        .flatten()
        .map(|(start, end)| match_value(captures.source.clone(), start, end))
        .map(Value::some)
        .unwrap_or_else(Value::none)
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
