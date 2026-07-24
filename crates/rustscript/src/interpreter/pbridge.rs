//! The parallel engine's bridge subset: format rendering, method and path
//! dispatch, iteration, and subprocess. It covers what fan-out scripts need,
//! for example gh-clone spawning many `git clone` processes, and bails with a
//! clear message on anything not yet ported.

use std::f64::consts::PI;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use parking_lot::Mutex;

use super::builtins::path_call_chunk;
use super::bytecode::MethodName;
use super::pchunk::{PChunk, convert};
use super::pnative::PNative;
use super::pvalue::{PClosure, PValue, PValueRef};
use super::pvm::PInterp;
use super::shared::{self, Args, CharOut, Num, NumOut, ParseNum, StrOut};

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
            Some("OS") if segs.iter().any(|segment| segment == "consts") => {
                Ok(PValue::str(std::env::consts::OS))
            }
            Some("ARCH") if segs.iter().any(|segment| segment == "consts") => {
                Ok(PValue::str(std::env::consts::ARCH))
            }
            Some(other) => {
                // A bare function name used as a value, `.map(strip_html)`. The
                // closure forwards its arguments to the call, which the tokio
                // `dispatch_call` resolves back to the user function.
                if segs.len() == 1
                    && let Some(chunk) = self.user_function(other)
                {
                    let inline = path_call_chunk(segs.to_vec(), chunk.num_params);
                    return Ok(PValue::Closure(Arc::new(PClosure {
                        chunk: convert(&inline),
                        captured: Vec::new(),
                    })));
                }
                bail!("unsupported path value `{other}` in tokio mode")
            }
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
            ("Regex", "new") => {
                let pattern = arg0(&args).display();
                match regex::Regex::new(&pattern) {
                    Ok(r) => Ok(PValue::ok(super::pregex::make_regex(r, &pattern))),
                    Err(e) => Ok(PValue::err(PValue::str(e.to_string()))),
                }
            }
            ("Stdio", "piped" | "inherit" | "null") => Ok(PValue::struct_of(
                "Stdio",
                [("kind".into(), PValue::str(last))],
            )),
            // A child's piped end is already buffered, so wrapping it is a no-op
            // that hands the same handle back.
            ("BufReader" | "BufWriter", "new" | "with_capacity") => {
                match args.into_iter().next_back() {
                    Some(v @ PValue::Native(_)) => Ok(v),
                    _ => bail!("BufReader::new needs a reader handle in tokio mode"),
                }
            }
            ("Vec", "new") | ("Vec", "with_capacity") => Ok(PValue::vec(vec![])),
            ("HashMap", "new") | ("HashMap", "with_capacity") | ("BTreeMap", "new") => {
                Ok(PValue::map())
            }
            ("String", "new") => Ok(PValue::str("")),
            ("String", "from") => Ok(PValue::str(arg0(&args).display())),
            ("Instant", "now") => Ok(PNative::Instant(Instant::now()).wrap()),
            ("Duration", "from_millis") => Ok(duration_value(int_arg(&args))),
            ("Duration", "from_secs") => Ok(duration_value(int_arg(&args).saturating_mul(1000))),
            ("process", "exit") => {
                let code = match args.first() {
                    Some(PValue::Int(i)) => *i as i32,
                    _ => 0,
                };
                std::process::exit(code)
            }
            ("time", "sleep") => Ok(sleep_future(&args)),
            ("task", "yield_now") => Ok(yield_future()),
            _ => {
                if let Some(v) = super::pstd::native_call(ns, last, &args)? {
                    return Ok(v);
                }
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
        let dereferenced = if let PValue::Ref(reference) = recv {
            let Some(value) = reference.get() else {
                bail!("method call through a dangling reference");
            };
            Some(value)
        } else {
            None
        };
        let recv = dereferenced.as_ref().unwrap_or(recv);
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
            // Option's closure-taking methods, `map` and friends, need the
            // interpreter to invoke a closure, so they route through `self`
            // here. Everything else on an enum falls to the plain `enum_method`.
            PValue::Enum {
                enum_name,
                variant,
                data,
            } if &**enum_name == "Option" => {
                match self.option_higher_order(variant, data, m, args)? {
                    Some(v) => Ok(v),
                    None => enum_method(recv, m, args),
                }
            }
            PValue::Enum { .. } => enum_method(recv, m, args),
            PValue::Struct(st) if &**st.name() == "Command" => {
                super::pprocess::command_method(recv, m, args)
            }
            PValue::Struct(st) if &**st.name() == "Child" => super::pprocess::child_method(recv, m),
            PValue::Struct(st) if &**st.name() == "ExitStatus" => exitstatus_method(st, m),
            PValue::Struct(st) if &**st.name() == "Output" => output_method(st, m),
            PValue::Struct(st) if &**st.name() == "Duration" => duration_method(st, m),
            PValue::Struct(st) if matches!(&**st.name(), "Path" | "PathBuf") => {
                super::pstd::path_method(st, m, args)
            }
            PValue::Struct(st) if &**st.name() == "OsString" => {
                super::pstd::os_string_method(st, m)
            }
            PValue::Struct(st) if &**st.name() == "DirEntry" => {
                super::pstd::dir_entry_method(st, m)
            }
            PValue::Struct(st) if &**st.name() == "FileType" => {
                super::pstd::file_type_method(st, m)
            }
            PValue::Struct(st) if &**st.name() == "Metadata" => super::pstd::metadata_method(st, m),
            PValue::Struct(st) if &**st.name() == "StdStream" => {
                super::pstd::std_stream_method(st, m)
            }
            PValue::Native(native) => native_method(native, m, args),
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
            // Compiled from `v[a..b].copy_from_slice(src)`, see the fast twin.
            "copy_from_slice" => {
                let start = usize::try_from(int_arg(args)).unwrap_or(0);
                let end_raw = match args.get(1) {
                    Some(PValue::Int(n)) => *n,
                    _ => bail!("copy_from_slice takes numeric bounds"),
                };
                let src: Vec<PValue> = match args.get(2) {
                    Some(PValue::Vec(other)) => other.lock().clone(),
                    _ => bail!("copy_from_slice takes a slice argument"),
                };
                let mut items = items.lock();
                let end = if end_raw == i64::MAX {
                    items.len()
                } else {
                    end_raw as usize
                };
                if end > items.len() {
                    bail!(
                        "range end index {end} out of range for slice of length {}",
                        items.len()
                    );
                }
                let dst_len = end.saturating_sub(start);
                if dst_len != src.len() {
                    bail!(
                        "source slice length ({}) does not match destination slice length ({dst_len})",
                        src.len()
                    );
                }
                for (k, val) in src.into_iter().enumerate() {
                    items[start + k] = val;
                }
                PValue::Unit
            }
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
            "iter" | "into_iter" | "collect" | "to_vec" | "copied" => {
                PValue::vec(items.lock().clone())
            }
            "iter_mut" => {
                let len = items.lock().len();
                PValue::vec(
                    (0..len)
                        .map(|index| {
                            PValue::Ref(Arc::new(PValueRef::vec_element(items.clone(), index)))
                        })
                        .collect(),
                )
            }
            "extend" | "extend_from_slice" => {
                // Clone the source first, so extending a vec with itself does
                // not deadlock on the same mutex.
                if let Some(PValue::Vec(other)) = args.first() {
                    let vals = other.lock().clone();
                    items.lock().extend(vals);
                }
                PValue::Unit
            }
            "nth" => {
                let index = match args.first() {
                    Some(PValue::Int(i)) => usize::try_from(*i).unwrap_or(0),
                    _ => 0,
                };
                items
                    .lock()
                    .get(index)
                    .cloned()
                    .map_or_else(PValue::none, PValue::some)
            }
            "collect_string" => {
                PValue::str(items.lock().iter().map(PValue::display).collect::<String>())
            }
            "sort" => {
                items.lock().sort_by(|a, b| {
                    super::pops::compare_values(a, b).unwrap_or(std::cmp::Ordering::Equal)
                });
                PValue::Unit
            }
            // A vec of vecs flattens like the real slice `concat`; anything
            // else concatenates the display forms, which covers `Vec<String>`.
            // The empty case cannot know its element type, so it is a string.
            "concat" => {
                let items = items.lock();
                match items.first() {
                    Some(PValue::Vec(_)) => {
                        let mut out = Vec::new();
                        for x in items.iter() {
                            if let PValue::Vec(inner) = x {
                                out.extend(inner.lock().iter().cloned());
                            }
                        }
                        PValue::vec(out)
                    }
                    _ => PValue::str(items.iter().map(PValue::display).collect::<String>()),
                }
            }
            "join" => {
                let sep = args.first().map(PValue::display).unwrap_or_default();
                let parts: Vec<String> = items.lock().iter().map(PValue::display).collect();
                PValue::str(parts.join(&sep))
            }
            // Higher order methods. The parallel engine keeps collections eager,
            // so each of these runs the closure over every item right here
            // rather than building a lazy iterator.
            "map" | "filter" | "filter_map" | "flat_map" | "any" | "all" | "find" | "position"
            | "for_each" => {
                let f = args.first().cloned().unwrap_or(PValue::Unit);
                let items = items.lock().clone();
                return self.higher_order(m, &items, &f);
            }
            "rev" => {
                let mut out = items.lock().clone();
                out.reverse();
                PValue::vec(out)
            }
            "enumerate" => PValue::vec(
                items
                    .lock()
                    .iter()
                    .enumerate()
                    .map(|(i, v)| PValue::tuple(vec![PValue::Int(i as i64), v.clone()]))
                    .collect(),
            ),
            "count" => PValue::Int(items.lock().len() as i64),
            "sum" => {
                let mut total = PValue::Int(0);
                for v in items.lock().iter() {
                    total = super::pops::apply_bin(super::bytecode::BinKind::Add, &total, v)?;
                }
                total
            }
            "max" | "min" => {
                let items = items.lock();
                let want = if m == "max" {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Less
                };
                let mut best: Option<PValue> = None;
                for v in items.iter() {
                    let take = match &best {
                        None => true,
                        Some(b) => super::pops::compare_values(v, b)? == want,
                    };
                    if take {
                        best = Some(v.clone());
                    }
                }
                best.map_or_else(PValue::none, PValue::some)
            }
            _ => bail!("method `{m}` on Vec is not supported in tokio mode"),
        })
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

/// The shape carries every field a later builder call can set, since a shape
/// cannot grow once the instance exists and `set` on an unknown field is a
/// silent no-op.
fn command_new(program: PValue) -> PValue {
    PValue::struct_of(
        "Command",
        [
            ("program".into(), PValue::str(program.display())),
            ("args".into(), PValue::vec(vec![])),
            ("current_dir".into(), PValue::Unit),
            ("envs".into(), PValue::Unit),
            ("stdin".into(), PValue::Unit),
            ("stdout".into(), PValue::Unit),
            ("stderr".into(), PValue::Unit),
        ],
    )
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
        "status" | "stdout" | "stderr" => s.get(m).unwrap_or(PValue::Unit),
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

fn native_method(native: &Arc<Mutex<PNative>>, m: &str, args: &mut [PValue]) -> Result<PValue> {
    if let PNative::Instant(instant) = &*native.lock()
        && m == "elapsed"
    {
        return Ok(duration_from_std(instant.elapsed()));
    }
    // The subprocess family: pipe readers, line iterators and the stdin writer.
    if let Some(v) = super::pprocess::native_method(native, m, args)? {
        return Ok(v);
    }
    if let Some(v) = super::pregex::regex_native_method(native, m, args)? {
        return Ok(v);
    }
    let native = native.lock();
    bail!(
        "method `{m}` on {} is not supported in tokio mode",
        native.type_name()
    )
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
    let n = match recv {
        PValue::Int(i) => Some(Num::Int(*i)),
        PValue::Float(f) => Some(Num::Float(*f)),
        _ => None,
    };
    if let Some(n) = n {
        if let Some(out) = shared::num_core(n, m, &PArgs(args))? {
            return Ok(num_out(out));
        }
        bail!("method `{m}` on a number is not supported in tokio mode");
    }
    if let PValue::Char(ch) = recv
        && let Some(out) = shared::char_method(*ch, m)
    {
        return Ok(match out {
            CharOut::Bool(v) => PValue::Bool(v),
            CharOut::Char(c) => PValue::Char(c),
            CharOut::Str(s) => PValue::str(s),
        });
    }
    bail!(
        "method `{m}` on {} is not supported in tokio mode",
        recv.type_name()
    )
}

/// Turn a neutral numeric core answer into a parallel engine value.
fn num_out(out: NumOut) -> PValue {
    match out {
        NumOut::Int(i) => PValue::Int(i),
        NumOut::Float(f) => PValue::Float(f),
        NumOut::Bool(b) => PValue::Bool(b),
        NumOut::SomeInt(i) => PValue::some(PValue::Int(i)),
        NumOut::SomeFloat(f) => PValue::some(PValue::Float(f)),
        NumOut::Nothing => PValue::none(),
        NumOut::Ordering(o) => p_ordering(o),
        NumOut::SomeOrdering(o) => PValue::some(p_ordering(o)),
    }
}

/// `std::cmp::Ordering` as the enum value scripts match on, the parallel twin
/// of the fast engine's `make_ordering`.
fn p_ordering(o: std::cmp::Ordering) -> PValue {
    let variant = match o {
        std::cmp::Ordering::Less => "Less",
        std::cmp::Ordering::Equal => "Equal",
        std::cmp::Ordering::Greater => "Greater",
    };
    PValue::Enum {
        enum_name: Arc::from("Ordering"),
        variant: Arc::from(variant),
        data: Arc::from(Vec::new()),
    }
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
                } else if &**enum_name == "Option" {
                    "called `Option::unwrap()` on a `None` value".to_string()
                } else {
                    format!("called `Result::unwrap()` on an `{variant}` value")
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
        // These borrow or move the payload in real Rust. The interpreter shares
        // values, so handing the same value back is equivalent, and it is what
        // the fast engine does too.
        "as_ref" | "as_deref" | "as_mut" | "take" | "cloned" | "copied" => recv.clone(),
        // `Option::context` and `Result::context` produce a Result, so a
        // following `?` has something to unwrap.
        "context" | "with_context" => {
            if matches!(&**variant, "Some" | "Ok") {
                PValue::ok(payload())
            } else {
                PValue::err(PValue::str(
                    args.first().map(PValue::display).unwrap_or_default(),
                ))
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
    if let Some(out) = shared::str_core(s, m, &PArgs(args))? {
        return Ok(str_out(s, out));
    }
    if let Some(text) = shared::color_core(s, m) {
        return Ok(PValue::str(text));
    }
    Ok(match m {
        // Eager forms of what the fast engine serves as lazy iterators.
        "split_whitespace" => PValue::vec(s.split_whitespace().map(PValue::str).collect()),
        "lines" => PValue::vec(s.lines().map(PValue::str).collect()),
        "chars" => PValue::vec(s.chars().map(PValue::Char).collect()),
        "bytes" => PValue::vec(s.bytes().map(|b| PValue::Int(i64::from(b))).collect()),
        _ => bail!("method `{m}` on String is not supported in tokio mode"),
    })
}

/// Turn a neutral string core answer into a parallel engine value. `Keep`
/// clones the `Arc`, so handing the receiver back stays a refcount bump.
fn str_out(s: &Arc<str>, out: StrOut) -> PValue {
    match out {
        StrOut::Bool(b) => PValue::Bool(b),
        StrOut::Int(i) => PValue::Int(i),
        StrOut::Owned(o) => PValue::str(o),
        StrOut::Keep => PValue::Str(s.clone()),
        StrOut::OkKeep => PValue::ok(PValue::Str(s.clone())),
        StrOut::Strs(v) => PValue::vec(v.into_iter().map(PValue::str).collect()),
        StrOut::CharIdx(v) => PValue::vec(
            v.into_iter()
                .map(|(i, c)| PValue::tuple(vec![PValue::Int(i), PValue::Char(c)]))
                .collect(),
        ),
        StrOut::Ints(v) => PValue::vec(v.into_iter().map(PValue::Int).collect()),
        StrOut::OptOwned(o) => match o {
            Some(x) => PValue::some(PValue::str(x)),
            None => PValue::none(),
        },
        StrOut::OptInt(o) => match o {
            Some(i) => PValue::some(PValue::Int(i)),
            None => PValue::none(),
        },
        StrOut::OptPair(o) => match o {
            Some((x, y)) => PValue::some(PValue::tuple(vec![PValue::str(x), PValue::str(y)])),
            None => PValue::none(),
        },
        StrOut::Ordering(o) => p_ordering(o),
        StrOut::Parse(p) => match p {
            ParseNum::Int(i) => PValue::ok(PValue::Int(i)),
            ParseNum::Float(f) => PValue::ok(PValue::Float(f)),
            ParseNum::Bool(b) => PValue::ok(PValue::Bool(b)),
            ParseNum::Fail(msg) => PValue::err(PValue::str(msg)),
        },
    }
}

/// The parallel engine's argument view for the shared cores.
struct PArgs<'a>(&'a [PValue]);

impl Args for PArgs<'_> {
    fn text(&self, i: usize) -> String {
        self.0.get(i).map(PValue::display).unwrap_or_default()
    }

    fn int(&self, i: usize) -> Option<i64> {
        match self.0.get(i) {
            Some(PValue::Int(n)) => Some(*n),
            _ => None,
        }
    }

    fn float(&self, i: usize) -> Option<f64> {
        match self.0.get(i) {
            Some(PValue::Float(f)) => Some(*f),
            Some(PValue::Int(n)) => Some(*n as f64),
            _ => None,
        }
    }

    fn pattern_chars(&self, i: usize) -> Option<Vec<char>> {
        let Some(PValue::Vec(items)) = self.0.get(i) else {
            return None;
        };
        Some(
            items
                .lock()
                .iter()
                .filter_map(|v| match v {
                    PValue::Char(c) => Some(*c),
                    PValue::Str(text) => text.chars().next(),
                    _ => None,
                })
                .collect(),
        )
    }
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
                let value = resolve_arg(name, &mut next_pos, positional, named)?;
                // A `{:w$}` width names another argument, so resolve it against
                // the same tables before the spec is applied.
                let mut lookup = |token: &str| -> Result<i64> {
                    let mut pos = 0;
                    match resolve_arg(token, &mut pos, positional, named)? {
                        PValue::Int(i) => Ok(i),
                        other => {
                            bail!("format width must be an integer, got {}", other.type_name())
                        }
                    }
                };
                let fmt = super::format::expand_widths_with(fmt, &mut lookup)?;
                let number = match &value {
                    PValue::Float(f) => Some(*f),
                    PValue::Int(i) => Some(*i as f64),
                    _ => None,
                };
                out.push_str(&super::format::apply_spec(
                    &fmt,
                    &value.display(),
                    &value.debug(),
                    number,
                ));
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

impl PInterp {
    /// Run a closure over every item for the higher order Vec methods. Kept in
    /// one place so map, filter and the predicates share their calling shape.
    fn higher_order(self: &Arc<Self>, m: &str, items: &[PValue], f: &PValue) -> Result<PValue> {
        let mut out = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let got = self.call_closure(f, std::slice::from_ref(item))?;
            match m {
                "map" => out.push(got),
                "for_each" => {}
                "filter" => {
                    if got.is_truthy() {
                        out.push(item.clone());
                    }
                }
                "filter_map" => {
                    if let PValue::Enum { variant, data, .. } = &got
                        && &**variant == "Some"
                    {
                        out.push(data.first().cloned().unwrap_or(PValue::Unit));
                    }
                }
                "flat_map" => match got {
                    PValue::Vec(inner) => out.extend(inner.lock().iter().cloned()),
                    other => out.push(other),
                },
                "any" => {
                    if got.is_truthy() {
                        return Ok(PValue::Bool(true));
                    }
                }
                "all" => {
                    if !got.is_truthy() {
                        return Ok(PValue::Bool(false));
                    }
                }
                "find" => {
                    if got.is_truthy() {
                        return Ok(PValue::some(item.clone()));
                    }
                }
                "position" => {
                    if got.is_truthy() {
                        return Ok(PValue::some(PValue::Int(index as i64)));
                    }
                }
                _ => bail!("unknown higher order method `{m}`"),
            }
        }
        Ok(match m {
            "any" => PValue::Bool(false),
            "all" => PValue::Bool(true),
            "find" | "position" => PValue::none(),
            "for_each" => PValue::Unit,
            _ => PValue::vec(out),
        })
    }

    /// Closure-taking methods on Option, the parallel-engine twin of
    /// `option_higher_order` in higher_order.rs. Returns None when `m` is not one
    /// of these, so the caller falls through to the non-closure `enum_method`.
    fn option_higher_order(
        self: &Arc<Self>,
        variant: &str,
        data: &Arc<[PValue]>,
        m: &str,
        args: &[PValue],
    ) -> Result<Option<PValue>> {
        let is_some = variant == "Some";
        let inner = || data.first().cloned().unwrap_or(PValue::Unit);
        let clo = |i: usize| -> Result<PValue> {
            match args.get(i) {
                Some(closure @ PValue::Closure(_)) => Ok(closure.clone()),
                _ => bail!("this method expects a closure argument"),
            }
        };
        let out = match m {
            "is_some_and" => {
                PValue::Bool(is_some && self.call_closure(&clo(0)?, &[inner()])?.is_truthy())
            }
            "map" => {
                if is_some {
                    PValue::some(self.call_closure(&clo(0)?, &[inner()])?)
                } else {
                    PValue::none()
                }
            }
            "and_then" => {
                if is_some {
                    self.call_closure(&clo(0)?, &[inner()])?
                } else {
                    PValue::none()
                }
            }
            "filter" => {
                if is_some && self.call_closure(&clo(0)?, &[inner()])?.is_truthy() {
                    PValue::some(inner())
                } else {
                    PValue::none()
                }
            }
            "map_or" => {
                let default = args.first().cloned().unwrap_or(PValue::Unit);
                if is_some {
                    self.call_closure(&clo(1)?, &[inner()])?
                } else {
                    default
                }
            }
            "map_or_else" => {
                if is_some {
                    self.call_closure(&clo(1)?, &[inner()])?
                } else {
                    self.call_closure(&clo(0)?, &[])?
                }
            }
            "unwrap_or_else" => {
                if is_some {
                    inner()
                } else {
                    self.call_closure(&clo(0)?, &[])?
                }
            }
            "ok_or_else" | "with_context" => {
                if is_some {
                    PValue::ok(inner())
                } else {
                    PValue::err(self.call_closure(&clo(0)?, &[])?)
                }
            }
            "or_else" => {
                if is_some {
                    PValue::some(inner())
                } else {
                    self.call_closure(&clo(0)?, &[])?
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(out))
    }
}
