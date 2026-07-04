//! The regex crate bridge. Split from `builtins.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Result, anyhow, bail};


use super::value::{Fields, KeyRef, Map, MapKey, RStr, Value};



// -- regex bridge ----------------------------------------------------------

pub(super) fn make_regex(pattern: String) -> Value {
    let mut f = Fields::default();
    f.insert("pattern".into(), Value::str(pattern));
    Value::Struct {
        name: "Regex".into(),
        fields: Rc::new(RefCell::new(f)),
    }
}

pub(super) fn make_match(m: &regex::Match) -> Value {
    let mut f = Fields::default();
    f.insert("text".into(), Value::str(m.as_str().to_string()));
    f.insert("start".into(), Value::Int(m.start() as i64));
    f.insert("end".into(), Value::Int(m.end() as i64));
    Value::Struct {
        name: "Match".into(),
        fields: Rc::new(RefCell::new(f)),
    }
}

pub(super) fn make_captures(re: &regex::Regex, caps: &regex::Captures) -> Value {
    let groups: Vec<Value> = (0..caps.len())
        .map(|i| match caps.get(i) {
            Some(m) => Value::some(make_match(&m)),
            None => Value::none(),
        })
        .collect();
    let mut names = Map::default();
    for (i, name) in re.capture_names().enumerate() {
        if let Some(n) = name {
            names.insert(MapKey::Str(RStr::new(n.to_string())), Value::Int(i as i64));
        }
    }
    let mut f = Fields::default();
    f.insert("groups".into(), Value::vec(groups));
    f.insert("names".into(), Value::Map(Rc::new(RefCell::new(names))));
    Value::Struct {
        name: "Captures".into(),
        fields: Rc::new(RefCell::new(f)),
    }
}

pub(super) fn regex_method(
    fields: &Rc<RefCell<Fields>>,
    method: &str,
    args: &[Value],
) -> Result<Value> {
    let pattern = fields.borrow().get("pattern").map(|v| v.display()).unwrap_or_default();
    let re = regex::Regex::new(&pattern)?;
    let text = args.first().map(|v| v.display()).unwrap_or_default();
    let rep = args.get(1).map(|v| v.display()).unwrap_or_default();
    Ok(match method {
        "is_match" => Value::Bool(re.is_match(&text)),
        "find" => match re.find(&text) {
            Some(m) => Value::some(make_match(&m)),
            None => Value::none(),
        },
        "find_iter" => Value::vec(re.find_iter(&text).map(|m| make_match(&m)).collect()),
        "captures" => match re.captures(&text) {
            Some(c) => Value::some(make_captures(&re, &c)),
            None => Value::none(),
        },
        "captures_iter" => Value::vec(
            re.captures_iter(&text)
                .map(|c| make_captures(&re, &c))
                .collect(),
        ),
        "replace" => Value::str(re.replacen(&text, 1, rep.as_str()).into_owned()),
        "replace_all" => Value::str(re.replace_all(&text, rep.as_str()).into_owned()),
        "split" => Value::vec(re.split(&text).map(Value::str).collect()),
        "as_str" => Value::str(pattern),
        _ => bail!("unknown method `{method}` on Regex"),
    })
}

pub(super) fn match_method(fields: &Rc<RefCell<Fields>>, method: &str) -> Result<Value> {
    let f = fields.borrow();
    Ok(match method {
        "as_str" => f.get("text").cloned().unwrap_or_else(|| Value::str("")),
        "start" => f.get("start").cloned().unwrap_or(Value::Int(0)),
        "end" => f.get("end").cloned().unwrap_or(Value::Int(0)),
        _ => bail!("unknown method `{method}` on Match"),
    })
}

pub(super) fn captures_method(
    fields: &Rc<RefCell<Fields>>,
    method: &str,
    args: &[Value],
) -> Result<Value> {
    match method {
        "get" => {
            let i = match args.first() {
                Some(Value::Int(n)) => *n as usize,
                _ => bail!("captures get needs an index"),
            };
            Ok(capture_group(fields, i))
        }
        "name" => {
            let name = args.first().map(|v| v.display()).unwrap_or_default();
            match capture_name_index(fields, &name) {
                Some(i) => Ok(capture_group(fields, i)),
                None => Ok(Value::none()),
            }
        }
        "len" => {
            if let Some(Value::Vec(g)) = fields.borrow().get("groups") {
                Ok(Value::Int(g.borrow().len() as i64))
            } else {
                Ok(Value::Int(0))
            }
        }
        _ => bail!("unknown method `{method}` on Captures"),
    }
}

pub(super) fn capture_group(fields: &Rc<RefCell<Fields>>, i: usize) -> Value {
    match fields.borrow().get("groups") {
        Some(Value::Vec(g)) => g.borrow().get(i).cloned().unwrap_or_else(Value::none),
        _ => Value::none(),
    }
}

pub(super) fn capture_name_index(fields: &Rc<RefCell<Fields>>, name: &str) -> Option<usize> {
    if let Some(Value::Map(names)) = fields.borrow().get("names")
        && let Some(Value::Int(i)) = names.borrow().get(&KeyRef::Str(name))
    {
        return Some(*i as usize);
    }
    None
}

/// Resolve `caps[i]` or `caps["name"]` to the matched substring, panicking like
/// the real `Captures` index does when the group did not participate.
pub(super) fn capture_index(
    fields: &Rc<RefCell<Fields>>,
    key: &Value,
) -> Result<Value> {
    let idx = match key {
        Value::Int(i) if *i >= 0 => *i as usize,
        Value::Str(s) => capture_name_index(fields, s)
            .ok_or_else(|| anyhow!("no capture group named `{s}`"))?,
        _ => bail!("invalid capture index"),
    };
    match capture_group(fields, idx) {
        Value::Enum { variant, data, .. } if &*variant == "Some" => {
            let m = data.first().cloned().unwrap_or(Value::Unit);
            if let Value::Struct { fields: mf, .. } = m {
                return Ok(mf.borrow().get("text").cloned().unwrap_or_else(|| Value::str("")));
            }
            bail!("bad capture group")
        }
        _ => bail!("no match for capture group {idx}"),
    }
}
