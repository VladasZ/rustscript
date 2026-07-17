//! The parallel engine's bridge subset: format rendering, method and path
//! dispatch, iteration, and subprocess. It covers what fan-out scripts need,
//! for example gh-clone spawning many `git clone` processes, and bails with a
//! clear message on anything not yet ported.

use std::f64::consts::PI;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use parking_lot::Mutex;

use super::bytecode::MethodName;
use super::pchunk::PChunk;
use super::pnative::PNative;
use super::pvalue::PValue;
use super::pvm::PInterp;

impl PInterp {
    // -- format ------------------------------------------------------------

    pub(super) fn render_fmt(&self, chunk: &PChunk, spec: u16, regs: &[PValue]) -> Result<String> {
        let f = &chunk.fmts[spec as usize];
        let positional: Vec<PValue> = f
            .positional
            .iter()
            .map(|r| regs[*r as usize].clone())
            .collect();
        let named: Vec<(&str, PValue)> = f
            .named
            .iter()
            .map(|(n, r)| (n.as_str(), regs[*r as usize].clone()))
            .collect();
        render_template(&f.template, &positional, &named)
    }

    // -- iteration ---------------------------------------------------------

    pub(super) fn iter_items(&self, v: PValue) -> Result<Vec<PValue>> {
        Ok(match v {
            PValue::Vec(items) => items.lock().clone(),
            PValue::Tuple(items) => items.lock().clone(),
            PValue::Range {
                start,
                end,
                inclusive,
            } => {
                let end = if inclusive { end + 1 } else { end };
                (start..end).map(PValue::Int).collect()
            }
            PValue::Str(s) => s.chars().map(PValue::Char).collect(),
            PValue::Map(m) => m
                .lock()
                .iter()
                .map(|(k, v)| PValue::tuple(vec![k.to_value(), v.clone()]))
                .collect(),
            other => bail!("cannot iterate a {}", other.type_name()),
        })
    }

    // -- path values -------------------------------------------------------

    pub(super) fn eval_path_value(&self, segs: &[String]) -> Result<PValue> {
        match segs.last().map(String::as_str) {
            Some("None") => Ok(PValue::none()),
            Some("PI") if segs.iter().any(|segment| segment == "consts") => Ok(PValue::Float(PI)),
            Some(other) => bail!("unsupported path value `{other}` in tokio mode"),
            None => bail!("empty path"),
        }
    }

    // -- path calls --------------------------------------------------------

    pub(super) fn dispatch_call(
        self: &Arc<Self>,
        segs: &[String],
        args: Vec<PValue>,
    ) -> Result<PValue> {
        if segs.first().map(String::as_str) == Some("reqwest") {
            return super::phttp::reqwest_call(segs, &args);
        }
        if segs.len() == 1 {
            return match segs[0].as_str() {
                "Some" => Ok(PValue::some(one(args)?)),
                "Ok" => Ok(PValue::ok(one(args)?)),
                "Err" => Ok(PValue::err(one(args)?)),
                "drop" => Ok(PValue::Unit),
                name => {
                    if let Some(chunk) = self.user_function(name) {
                        return self.run_chunk(&chunk, &args, &[]);
                    }
                    bail!("unknown function `{name}` in tokio mode")
                }
            };
        }
        let last = segs[segs.len() - 1].as_str();
        let ns = segs[segs.len() - 2].as_str();
        match (ns, last) {
            ("env", "args") => Ok(PValue::vec(
                super::script_args().into_iter().map(PValue::str).collect(),
            )),
            ("Command", "new") => Ok(command_new(args.into_iter().next().unwrap_or(PValue::Unit))),
            ("Vec", "new") | ("Vec", "with_capacity") => Ok(PValue::vec(vec![])),
            ("HashMap", "new") | ("HashMap", "with_capacity") | ("BTreeMap", "new") => {
                Ok(PValue::map())
            }
            ("String", "new") => Ok(PValue::str("")),
            ("String", "from") => Ok(PValue::str(arg0(&args).display())),
            ("Instant", "now") => Ok(PNative::Instant(Instant::now()).wrap()),
            ("Duration", "from_millis") => Ok(duration_value(int_arg(&args))),
            ("Duration", "from_secs") => Ok(duration_value(int_arg(&args).saturating_mul(1000))),
            ("time", "sleep") => Ok(sleep_future(&args)),
            ("task", "yield_now") => Ok(yield_future()),
            _ => {
                // A user associated function like `Type::new`, keyed by type.
                if let Some(chunk) = self.user_method(ns, last) {
                    return self.run_chunk(&chunk, &args, &[]);
                }
                // A user associated function or method by name, called UFCS.
                if let Some(chunk) = self.user_function(last) {
                    return self.run_chunk(&chunk, &args, &[]);
                }
                if let Some((recv, rest)) = args.split_first() {
                    let recv = recv.clone();
                    let mut rest = rest.to_vec();
                    let name = MethodName {
                        id: super::bytecode::BuiltinId::resolve(last),
                        text: last.to_string(),
                    };
                    return self.eval_method(&recv, &name, &mut rest);
                }
                bail!("unsupported call `{}` in tokio mode", segs.join("::"))
            }
        }
    }

    // -- methods -----------------------------------------------------------

    pub(super) fn eval_method(
        self: &Arc<Self>,
        recv: &PValue,
        name: &MethodName,
        args: &mut [PValue],
    ) -> Result<PValue> {
        let m = name.text.as_str();
        // A user defined `impl` method takes priority on a struct or enum, so a
        // script's own method is not shadowed by a builtin of the same name.
        let type_key = match recv {
            PValue::Struct(st) => Some(st.name().to_string()),
            PValue::Enum { enum_name, .. } => Some(enum_name.to_string()),
            _ => None,
        };
        if let Some(tk) = &type_key
            && let Some(chunk) = self.user_method(tk, m)
        {
            let mut full = Vec::with_capacity(args.len() + 1);
            full.push(recv.clone());
            full.extend(args.iter().cloned());
            return self.run_chunk(&chunk, &full, &[]);
        }
        // Methods available on any value.
        match m {
            "clone" => return Ok(recv.clone()),
            "to_string" => return Ok(PValue::str(recv.display())),
            _ => {}
        }
        // The async http client, request, and response types.
        if let Some(res) = super::phttp::http_method(recv, m, args) {
            return res;
        }
        match recv {
            PValue::Str(s) => str_method(s, m, args),
            PValue::Vec(_) => self.vec_method(recv, m, args),
            PValue::Map(_) => map_method(recv, m, args),
            PValue::Enum { .. } => enum_method(recv, m, args),
            PValue::Struct(st) if &**st.name() == "Command" => self.command_method(recv, m, args),
            PValue::Struct(st) if &**st.name() == "ExitStatus" => exitstatus_method(st, m),
            PValue::Struct(st) if &**st.name() == "Output" => output_method(st, m),
            PValue::Struct(st) if &**st.name() == "Duration" => duration_method(st, m),
            PValue::Native(native) => native_method(native, m),
            PValue::Int(_) | PValue::Float(_) | PValue::Bool(_) | PValue::Char(_) => {
                scalar_method(recv, m, args)
            }
            other => bail!(
                "method `{m}` on {} is not supported in tokio mode",
                other.type_name()
            ),
        }
    }

    fn vec_method(self: &Arc<Self>, recv: &PValue, m: &str, args: &mut [PValue]) -> Result<PValue> {
        let PValue::Vec(items) = recv else {
            unreachable!()
        };
        Ok(match m {
            "push" => {
                items
                    .lock()
                    .push(args.first().cloned().unwrap_or(PValue::Unit));
                PValue::Unit
            }
            "pop" => match items.lock().pop() {
                Some(v) => PValue::some(v),
                None => PValue::none(),
            },
            "len" => PValue::Int(items.lock().len() as i64),
            "is_empty" => PValue::Bool(items.lock().is_empty()),
            "clear" => {
                items.lock().clear();
                PValue::Unit
            }
            "first" => items
                .lock()
                .first()
                .cloned()
                .map_or_else(PValue::none, PValue::some),
            "last" => items
                .lock()
                .last()
                .cloned()
                .map_or_else(PValue::none, PValue::some),
            "contains" => {
                let needle = args.first().cloned().unwrap_or(PValue::Unit);
                PValue::Bool(items.lock().iter().any(|v| v.eq_value(&needle)))
            }
            "iter" | "into_iter" | "collect" | "to_vec" => PValue::vec(items.lock().clone()),
            "sort" => {
                items.lock().sort_by(|a, b| {
                    super::pops::compare_values(a, b).unwrap_or(std::cmp::Ordering::Equal)
                });
                PValue::Unit
            }
            "join" => {
                let sep = args.first().map(PValue::display).unwrap_or_default();
                let parts: Vec<String> = items.lock().iter().map(PValue::display).collect();
                PValue::str(parts.join(&sep))
            }
            _ => bail!("method `{m}` on Vec is not supported in tokio mode"),
        })
    }

    // -- subprocess --------------------------------------------------------

    fn command_method(
        self: &Arc<Self>,
        recv: &PValue,
        m: &str,
        args: &mut [PValue],
    ) -> Result<PValue> {
        let PValue::Struct(s) = recv else {
            unreachable!()
        };
        match m {
            "arg" => {
                push_arg(recv, args.first().cloned().unwrap_or(PValue::Unit));
                Ok(recv.clone())
            }
            "args" => {
                if let Some(PValue::Vec(list)) = args.first() {
                    for a in list.lock().iter() {
                        push_arg(recv, a.clone());
                    }
                }
                Ok(recv.clone())
            }
            "current_dir" => {
                s.set("current_dir", args.first().cloned().unwrap_or(PValue::Unit));
                Ok(recv.clone())
            }
            "status" => Ok(run_command(s, false)),
            "output" => Ok(run_command(s, true)),
            _ => bail!("method `{m}` on Command is not supported in tokio mode"),
        }
    }
}

// -- free helpers ----------------------------------------------------------

fn one(args: Vec<PValue>) -> Result<PValue> {
    args.into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("expected one argument"))
}

fn arg0(args: &[PValue]) -> PValue {
    args.first().cloned().unwrap_or(PValue::Unit)
}

fn command_new(program: PValue) -> PValue {
    PValue::struct_of(
        "Command",
        [
            ("program".into(), PValue::str(program.display())),
            ("args".into(), PValue::vec(vec![])),
            ("current_dir".into(), PValue::Unit),
        ],
    )
}

fn push_arg(cmd: &PValue, a: PValue) {
    if let PValue::Struct(s) = cmd
        && let Some(PValue::Vec(list)) = s.get("args")
    {
        list.lock().push(PValue::str(a.display()));
    }
}

fn run_command(s: &Arc<super::pvalue::PStructData>, capture: bool) -> PValue {
    let program = s.get("program").map(|v| v.display()).unwrap_or_default();
    let mut cmd = std::process::Command::new(&program);
    if let Some(PValue::Vec(list)) = s.get("args") {
        for a in list.lock().iter() {
            cmd.arg(a.display());
        }
    }
    if let Some(dir) = s.get("current_dir")
        && !matches!(dir, PValue::Unit)
    {
        cmd.current_dir(dir.display());
    }
    if capture {
        match cmd.output() {
            Ok(out) => PValue::ok(PValue::struct_of(
                "Output",
                [
                    (
                        "status".into(),
                        exit_status(out.status.success(), out.status.code()),
                    ),
                    ("stdout".into(), byte_vec(&out.stdout)),
                    ("stderr".into(), byte_vec(&out.stderr)),
                ],
            )),
            Err(e) => PValue::err(PValue::str(e.to_string())),
        }
    } else {
        match cmd.status() {
            Ok(st) => PValue::ok(exit_status(st.success(), st.code())),
            Err(e) => PValue::err(PValue::str(e.to_string())),
        }
    }
}

fn exit_status(success: bool, code: Option<i32>) -> PValue {
    PValue::struct_of(
        "ExitStatus",
        [
            ("success".into(), PValue::Bool(success)),
            ("code".into(), PValue::Int(code.unwrap_or(-1) as i64)),
        ],
    )
}

fn byte_vec(bytes: &[u8]) -> PValue {
    PValue::vec(bytes.iter().map(|&b| PValue::Int(b as i64)).collect())
}

fn exitstatus_method(s: &Arc<super::pvalue::PStructData>, m: &str) -> Result<PValue> {
    Ok(match m {
        "success" => s.get("success").unwrap_or(PValue::Bool(false)),
        "code" => match s.get("code") {
            Some(PValue::Int(c)) => PValue::some(PValue::Int(c)),
            _ => PValue::none(),
        },
        _ => bail!("method `{m}` on ExitStatus is not supported in tokio mode"),
    })
}

fn output_method(s: &Arc<super::pvalue::PStructData>, m: &str) -> Result<PValue> {
    Ok(match m {
        "status" => s.get("status").unwrap_or(PValue::Unit),
        _ => bail!("method `{m}` on Output is not supported in tokio mode"),
    })
}

fn int_arg(args: &[PValue]) -> i64 {
    match args.first() {
        Some(PValue::Int(i)) => *i,
        _ => 0,
    }
}

fn duration_value(millis: i64) -> PValue {
    duration_from_std(Duration::from_millis(millis as u64))
}

fn duration_from_std(duration: Duration) -> PValue {
    PValue::struct_of(
        "Duration",
        [
            ("millis".into(), PValue::Int(duration.as_millis() as i64)),
            ("nanos".into(), PValue::Int(duration.as_nanos() as i64)),
        ],
    )
}

fn sleep_future(args: &[PValue]) -> PValue {
    let millis = duration_millis(args.first());
    PNative::Future(Box::pin(async move {
        tokio::time::sleep(std::time::Duration::from_millis(millis)).await;
        PValue::Unit
    }))
    .wrap()
}

fn yield_future() -> PValue {
    PNative::Future(Box::pin(async {
        tokio::task::yield_now().await;
        PValue::Unit
    }))
    .wrap()
}

/// A Duration argument is modeled as a struct with a `millis` field, or falls
/// back to zero.
fn duration_millis(v: Option<&PValue>) -> u64 {
    match v {
        Some(PValue::Struct(s)) => match s.get("millis") {
            Some(PValue::Int(m)) => m as u64,
            _ => 0,
        },
        _ => 0,
    }
}

fn duration_method(s: &Arc<super::pvalue::PStructData>, m: &str) -> Result<PValue> {
    let nanos = match s.get("nanos") {
        Some(PValue::Int(nanos)) => nanos,
        _ => 0,
    };
    Ok(match m {
        "as_nanos" => PValue::Int(nanos),
        "as_micros" => PValue::Int(nanos / 1_000),
        "as_millis" => PValue::Int(nanos / 1_000_000),
        "as_secs" => PValue::Int(nanos / 1_000_000_000),
        _ => bail!("method `{m}` on Duration is not supported in tokio mode"),
    })
}

fn native_method(native: &Arc<Mutex<PNative>>, m: &str) -> Result<PValue> {
    let native = native.lock();
    match (&*native, m) {
        (PNative::Instant(instant), "elapsed") => Ok(duration_from_std(instant.elapsed())),
        (value, _) => bail!(
            "method `{m}` on {} is not supported in tokio mode",
            value.type_name()
        ),
    }
}

fn map_method(recv: &PValue, m: &str, args: &mut [PValue]) -> Result<PValue> {
    let PValue::Map(map) = recv else {
        unreachable!()
    };
    let key = |args: &mut [PValue]| args.first().and_then(PValue::as_key);
    Ok(match m {
        "insert" => {
            let k = key(args).ok_or_else(|| anyhow::anyhow!("invalid map key"))?;
            let v = args.get(1).cloned().unwrap_or(PValue::Unit);
            match map.lock().insert(k, v) {
                Some(old) => PValue::some(old),
                None => PValue::none(),
            }
        }
        "get" => {
            let k = key(args).ok_or_else(|| anyhow::anyhow!("invalid map key"))?;
            map.lock()
                .get(&k)
                .cloned()
                .map_or_else(PValue::none, PValue::some)
        }
        "contains_key" => {
            let k = key(args).ok_or_else(|| anyhow::anyhow!("invalid map key"))?;
            PValue::Bool(map.lock().contains_key(&k))
        }
        "remove" => {
            let k = key(args).ok_or_else(|| anyhow::anyhow!("invalid map key"))?;
            map.lock()
                .shift_remove(&k)
                .map_or_else(PValue::none, PValue::some)
        }
        "len" => PValue::Int(map.lock().len() as i64),
        "is_empty" => PValue::Bool(map.lock().is_empty()),
        "keys" => PValue::vec(map.lock().keys().map(|k| k.to_value()).collect()),
        "values" => PValue::vec(map.lock().values().cloned().collect()),
        _ => bail!("method `{m}` on HashMap is not supported in tokio mode"),
    })
}

fn scalar_method(recv: &PValue, m: &str, args: &[PValue]) -> Result<PValue> {
    Ok(match m {
        "abs" => match recv {
            PValue::Int(i) => PValue::Int(i.abs()),
            PValue::Float(f) => PValue::Float(f.abs()),
            _ => bail!("abs on non number"),
        },
        "is_multiple_of" => match recv {
            PValue::Int(value) => PValue::Bool(value % int_arg(args) == 0),
            _ => bail!("is_multiple_of on non integer"),
        },
        "is_sign_positive" => match recv {
            PValue::Float(value) => PValue::Bool(value.is_sign_positive()),
            _ => bail!("is_sign_positive on non float"),
        },
        "is_ascii_digit" => match recv {
            PValue::Char(ch) => PValue::Bool(ch.is_ascii_digit()),
            _ => bail!("is_ascii_digit on non char"),
        },
        _ => bail!(
            "method `{m}` on {} is not supported in tokio mode",
            recv.type_name()
        ),
    })
}

fn enum_method(recv: &PValue, m: &str, args: &mut [PValue]) -> Result<PValue> {
    let PValue::Enum {
        enum_name,
        variant,
        data,
    } = recv
    else {
        unreachable!()
    };
    let payload = || data.first().cloned().unwrap_or(PValue::Unit);
    Ok(match m {
        "unwrap" | "expect" => {
            if matches!(&**variant, "Some" | "Ok") {
                payload()
            } else {
                let msg = if m == "expect" {
                    args.first().map(PValue::display).unwrap_or_default()
                } else {
                    format!("called unwrap on a {variant} value")
                };
                bail!("{msg}");
            }
        }
        "unwrap_or" => {
            if matches!(&**variant, "Some" | "Ok") {
                payload()
            } else {
                args.first().cloned().unwrap_or(PValue::Unit)
            }
        }
        "unwrap_or_default" => {
            if matches!(&**variant, "Some" | "Ok") {
                payload()
            } else {
                PValue::Unit
            }
        }
        "is_some" => PValue::Bool(&**variant == "Some"),
        "is_none" => PValue::Bool(&**variant == "None"),
        "is_ok" => PValue::Bool(&**variant == "Ok"),
        "is_err" => PValue::Bool(&**variant == "Err"),
        "ok" => {
            if &**variant == "Ok" {
                PValue::some(payload())
            } else {
                PValue::none()
            }
        }
        _ => bail!("method `{m}` on {enum_name} is not supported in tokio mode"),
    })
}

fn str_method(s: &Arc<str>, m: &str, args: &mut [PValue]) -> Result<PValue> {
    let a0 = || args.first().map(PValue::display).unwrap_or_default();
    Ok(match m {
        "len" => PValue::Int(s.len() as i64),
        "is_empty" => PValue::Bool(s.is_empty()),
        "trim" => PValue::str(s.trim()),
        "to_string" | "as_str" | "to_owned" => PValue::str(&**s),
        "to_lowercase" | "to_ascii_lowercase" => PValue::str(s.to_lowercase()),
        "to_uppercase" | "to_ascii_uppercase" => PValue::str(s.to_uppercase()),
        "contains" => PValue::Bool(s.contains(&a0())),
        "starts_with" => PValue::Bool(s.starts_with(&a0())),
        "ends_with" => PValue::Bool(s.ends_with(&a0())),
        "replace" => {
            let from = args.first().map(PValue::display).unwrap_or_default();
            let to = args.get(1).map(PValue::display).unwrap_or_default();
            PValue::str(s.replace(&from, &to))
        }
        "replacen" => {
            let from = args.first().map(PValue::display).unwrap_or_default();
            let to = args.get(1).map(PValue::display).unwrap_or_default();
            let count = match args.get(2) {
                Some(PValue::Int(count)) => *count as usize,
                _ => 0,
            };
            PValue::str(s.replacen(&from, &to, count))
        }
        "split" => {
            let sep = a0();
            PValue::vec(s.split(&sep).map(PValue::str).collect())
        }
        "split_whitespace" => PValue::vec(s.split_whitespace().map(PValue::str).collect()),
        "lines" => PValue::vec(s.lines().map(PValue::str).collect()),
        "chars" => PValue::vec(s.chars().map(PValue::Char).collect()),
        "trim_end" => PValue::str(s.trim_end()),
        "trim_start" => PValue::str(s.trim_start()),
        "parse" => {
            let value = s.trim();
            if let Ok(value) = value.parse::<i64>() {
                PValue::ok(PValue::Int(value))
            } else if let Ok(value) = value.parse::<f64>() {
                PValue::ok(PValue::Float(value))
            } else if let Ok(value) = value.parse::<bool>() {
                PValue::ok(PValue::Bool(value))
            } else {
                PValue::err(PValue::str(format!("cannot parse `{value}`")))
            }
        }
        _ => bail!("method `{m}` on String is not supported in tokio mode"),
    })
}

// -- template rendering ----------------------------------------------------

fn render_template(
    template: &str,
    positional: &[PValue],
    named: &[(&str, PValue)],
) -> Result<String> {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    let mut next_pos = 0usize;
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut spec = String::new();
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                    spec.push(c);
                }
                let (name, fmt) = spec.split_once(':').unwrap_or((&spec, ""));
                let debug = fmt.contains('?');
                let value = resolve_arg(name, &mut next_pos, positional, named)?;
                if debug {
                    out.push_str(&value.debug());
                } else {
                    out.push_str(&value.display());
                }
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

fn resolve_arg(
    name: &str,
    next_pos: &mut usize,
    positional: &[PValue],
    named: &[(&str, PValue)],
) -> Result<PValue> {
    if name.is_empty() {
        let i = *next_pos;
        *next_pos += 1;
        return positional
            .get(i)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("format argument {i} is missing"));
    }
    if let Ok(i) = name.parse::<usize>() {
        return positional
            .get(i)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("format argument {i} is missing"));
    }
    named
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v.clone())
        .ok_or_else(|| anyhow::anyhow!("format name `{name}` is missing"))
}
