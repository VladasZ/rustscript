//! The bridge front door: format rendering, method and path
//! dispatch. The per-receiver method families live in `methods`, `vecmap`,
//! `higher_order`, and `iterator`; this file routes to them.

use num_traits::AsPrimitive;
use std::f64::consts::PI;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use parking_lot::Mutex;

use super::bytecode::Chunk;
use super::bytecode::{BuiltinId, MethodName};
use super::methods::{self, make_ordering};
use super::native::Native;
use super::shared::{self, Args, CharOut, F32Out, Num, NumOut, parse_rfc3339};
use super::value::{ClosureData, Value};
use super::vm::Vm;

impl Vm {
    // -- format ------------------------------------------------------------

    pub(super) fn render_fmt(
        self: &Arc<Self>,
        chunk: &Chunk,
        spec: u16,
        regs: &[Value],
    ) -> Result<String> {
        let f = &chunk.fmts[spec as usize];
        let positional: Vec<Value> = f
            .positional
            .iter()
            .map(|r| regs[*r as usize].clone())
            .collect();
        let named: Vec<(&str, Value)> = f
            .named
            .iter()
            .map(|(n, r)| (n.as_str(), regs[*r as usize].clone()))
            .collect();
        render_template(self, &f.template, &positional, &named)
    }

    /// The text a user `Display` or `Debug` impl renders for this value, or
    /// None when the value's type has no such impl. The impl runs with the
    /// value and a formatter buffer, and the buffer is the answer.
    pub(super) fn user_fmt_text(self: &Arc<Self>, v: &Value, key: &str) -> Result<Option<String>> {
        let ty: &str = match v {
            Value::Struct(s) => s.name(),
            Value::Enum { enum_name, .. } => enum_name,
            _ => return Ok(None),
        };
        let Some(chunk) = self.methods.get(&(ty.to_string(), key.to_string())) else {
            return Ok(None);
        };
        let chunk = chunk.clone();
        let handle = Arc::new(parking_lot::Mutex::new(Native::Fmt(String::new())));
        let args = vec![v.clone(), Value::Native(handle.clone())];
        self.run_chunk(&chunk, &args, &[])?;
        let text = match &*handle.lock() {
            Native::Fmt(s) => s.clone(),
            _ => String::new(),
        };
        Ok(Some(text))
    }

    /// Run the value's user `Drop::drop` when this is its last holder.
    /// Another holder means the value was moved or is still shared, and the
    /// real owner drops it at its own end of life.
    pub(super) fn run_user_drop(self: &Arc<Self>, value: Value) -> Result<()> {
        let (ty, unique) = match &value {
            Value::Struct(s) => (s.name().to_string(), Arc::strong_count(s) == 1),
            Value::Enum {
                enum_name, data, ..
            } => (enum_name.to_string(), Arc::strong_count(data) == 1),
            _ => return Ok(()),
        };
        if !unique {
            return Ok(());
        }
        let Some(chunk) = self.methods.get(&(ty, "Drop::drop".to_string())) else {
            return Ok(());
        };
        let chunk = chunk.clone();
        self.run_chunk(&chunk, &[value], &[])?;
        Ok(())
    }

    // -- path values -------------------------------------------------------

    pub(super) fn eval_path_value(&self, raw: &[String]) -> Result<Value> {
        let segs = self.canonical(raw);
        match segs.last().map(String::as_str) {
            Some("None") => Ok(Value::none()),
            Some("UNIX_EPOCH") => Ok(Native::SystemTime(std::time::UNIX_EPOCH).wrap()),
            Some(other) => {
                if let Some(v) = super::crates_bridge::base64_engine(other) {
                    return Ok(v);
                }
                if let Some(v) = super::winreg_bridge::winreg_const(other) {
                    return Ok(v);
                }
                if let Some(v) = super::service_bridge::service_const(other) {
                    return Ok(v);
                }
                if segs.len() >= 2 {
                    let ty = segs[segs.len() - 2].as_str();
                    if let Some(v) = typed_path_constant(ty, other) {
                        return Ok(v);
                    }
                    if let Some(v) = self.unit_variant(Some(ty), other) {
                        return Ok(v);
                    }
                } else {
                    if let Some(v) = self.unit_variant(None, other) {
                        return Ok(v);
                    }
                    // A unit struct used as a value, `struct Marker;` then
                    // `Marker`.
                    if let Some(name) = self
                        .unit_structs
                        .iter()
                        .find(|name| &***name == other || super::resolver::bare(name) == other)
                    {
                        return Ok(Value::structure(
                            super::value::StructShape::new(&**name, Vec::new()),
                            Vec::new(),
                        ));
                    }
                }
                // A bare function name used as a value, `.map(strip_html)`. The
                // closure forwards its arguments to the call, which
                // `dispatch_call` resolves back to the user function.
                if segs.len() == 1
                    && let Some(chunk) = self.user_function(other)
                {
                    return Ok(path_closure(segs.clone(), chunk.num_params));
                }
                // A path used as a function value. A zero-arg constructor like
                // `Vec::new` handed to `or_insert_with` becomes a nullary
                // closure. Anything else, a method reference like
                // `Value::as_str` handed to `and_then`, becomes a one-arg
                // closure, and `dispatch_call` resolves it as a UFCS method
                // call on that argument.
                if matches!(other, "new" | "default") {
                    return Ok(path_closure(segs.clone(), 0));
                }
                let function = segs.join("::");
                if segs.len() >= 2
                    && let Some(chunk) = self
                        .user_method(&segs[segs.len() - 2], other)
                        .or_else(|| self.user_function(&function))
                        .or_else(|| self.user_function(other))
                {
                    return Ok(path_closure(segs.clone(), chunk.num_params));
                }
                // A SCREAMING_CASE tail is a constant, never a function, so
                // wrapping it in the closure fallback would smuggle a closure
                // value into arithmetic.
                if other.chars().any(|c| c.is_ascii_uppercase())
                    && !other.chars().any(|c| c.is_ascii_lowercase())
                {
                    bail!("unsupported constant `{function}`");
                }
                Ok(path_closure(segs.clone(), 1))
            }
            None => bail!("empty path"),
        }
    }

    // -- path calls --------------------------------------------------------

    /// A one-segment call: the option and result constructors, `drop`, a
    /// script function by bare name, or a tuple struct or variant.
    fn dispatch_bare_call(self: &Arc<Self>, name: &str, args: Vec<Value>) -> Result<Value> {
        match name {
            "Some" => return Ok(Value::some(one(args)?)),
            "Ok" => return Ok(Value::ok(one(args)?)),
            "Err" => return Ok(Value::err(one(args)?)),
            // A user type with a `Drop` impl runs it now, its register was
            // cleared at the call site so this is the last holder. Anything
            // else dies with its register, file writes are unbuffered.
            "drop" => {
                self.run_user_drop(one(args)?)?;
                return Ok(Value::Unit);
            }
            _ => {}
        }
        if let Some(chunk) = self.user_function(name) {
            return self.run_chunk(&chunk, &args, &[]);
        }
        if self.struct_names.contains(name) {
            return Ok(Self::make_tuple_struct(name, args));
        }
        if let Some(v) = self.make_tuple_variant(None, name, &args) {
            return Ok(v);
        }
        bail!("unknown function `{name}`");
    }

    pub(super) fn dispatch_call(
        self: &Arc<Self>,
        segs: &[String],
        args: Vec<Value>,
    ) -> Result<Value> {
        let canon = self.canonical(segs);
        if canon.len() == 1 {
            return self.dispatch_bare_call(&canon[0], args);
        }

        // A namespaced call, `module::func` or `Type::func`. Match on the last
        // two segments so `use` shortenings and full paths behave the same.
        let last = canon[canon.len() - 1].as_str();
        let namespace = canon[canon.len() - 2].as_str();
        // The Ctrl-C handler must reach back into the interpreter to run the
        // script's own closure, so it cannot go through the plain native call.
        if namespace == "ctrlc" && last == "set_handler" {
            let closure = args.first().cloned().unwrap_or(Value::Unit);
            return Ok(match super::set_ctrlc_handler(closure) {
                Ok(()) => Value::ok(Value::Unit),
                Err(e) => Value::err(Value::str(e.to_string())),
            });
        }
        if namespace == "thread" {
            // sleep is the one thread function that needs no threading.
            if last == "sleep" {
                let Some(d) = args
                    .first()
                    .and_then(super::std_bridge::duration_from_value)
                else {
                    bail!("thread::sleep takes a Duration");
                };
                std::thread::sleep(d);
                return Ok(Value::Unit);
            }
            bail!("std::thread is not supported beyond sleep, use tokio::spawn");
        }
        // Namespaced calls reaching native bridges compute in i64 and f64, so
        // width-tagged numbers pass those their plain image. The script-level
        // targets below, a user chunk, a variant constructor, a payload
        // passthrough, or the UFCS method fallback, hand values through, so
        // they keep the real args and their width tags.
        let images: Vec<Value> = args
            .iter()
            .map(|arg| match arg.bridge_image() {
                Some(image) => image,
                None => arg.clone(),
            })
            .collect();
        if canon.first().map(String::as_str) == Some("reqwest") {
            return super::http::reqwest_call(&canon, &images);
        }
        // Ahead of the match because `Widget::render` mutates the buffer it is
        // given, so it must run exactly once.
        if let Some(v) = super::ratatui::ratatui_assoc(namespace, last, &images) {
            return Ok(v);
        }
        match (namespace, last) {
            ("serde_json", _) => super::json_bridge::bridge_serde_json(last, &images),
            ("Utc" | "Local", "now") => Ok(now_datetime(namespace == "Local")),
            ("DateTime", "parse_from_rfc3339") => {
                Ok(match parse_rfc3339(&arg0(&images).display()) {
                    Ok((unix_secs, nanos, offset)) => {
                        Value::ok(datetime_value(unix_secs, nanos, false, offset))
                    }
                    Err(e) => Value::err(Value::str(e)),
                })
            }
            ("time", "sleep") => Ok(sleep_future(&images)),
            ("task", "yield_now") => Ok(yield_future()),
            _ => {
                if let Some(chunk) = self.user_function(&canon.join("::")) {
                    return self.run_chunk(&chunk, &args, &[]);
                }
                if let Some(v) = super::std_bridge::native_call(namespace, last, &images)? {
                    return Ok(v);
                }
                // A method on a user type, `Type::assoc(..)` or UFCS
                // `Type::method(recv, ..)`. The receiver, if any, is simply
                // the first argument, matching param 0.
                if let Some(chunk) = self.user_method(namespace, last) {
                    return self.run_chunk(&chunk, &args, &[]);
                }
                if let Some(v) = super::assoc::assoc_fn(namespace, last, &images)? {
                    return Ok(v);
                }
                if let Some(v) = self.make_tuple_variant(Some(namespace), last, &args) {
                    return Ok(v);
                }
                // A json value written out in a script, `Value::String(s)`. A
                // parsed json is held as native values here, so each variant is
                // exactly its own payload.
                if namespace == "Value"
                    && matches!(last, "String" | "Bool" | "Number" | "Array" | "Object")
                {
                    return one(args);
                }
                // UFCS fallback: `Type::method(recv, ..)` dispatches `method`
                // on the receiver. This is what makes a method reference used
                // as a value, like `str::trim` handed to `map`, callable.
                // `eval_method` takes the real args, it images them itself
                // where a method needs that, so a width-aware method like
                // `u8::saturating_add` still sees its real width.
                if let Some((recv, rest)) = args.split_first() {
                    let recv = recv.clone();
                    let mut rest = rest.to_vec();
                    let name = MethodName {
                        id: BuiltinId::resolve(last),
                        text: last.to_string(),
                        scalar: None,
                    };
                    return self.eval_method(&recv, &name, &mut rest);
                }
                bail!("unsupported call `{}`", canon.join("::"))
            }
        }
    }

    // -- methods -----------------------------------------------------------

    /// The dispatch steps that run before any per-receiver bridge: a shared
    /// cell's own wrapper methods and its auto-deref, `to_string` through a
    /// user `Display` impl, and the width-tagged number shortcuts.
    fn pre_dispatch(
        self: &Arc<Self>,
        recv: &Value,
        name: &MethodName,
        args: &mut [Value],
    ) -> Result<Option<Value>> {
        if let Value::Cell(kind, slot) = recv {
            if let Some(v) = super::cell::cell_method(*kind, slot, &name.text, args)? {
                return Ok(Some(v));
            }
            let inner = slot.lock().clone();
            return self.eval_method(&inner, name, args).map(Some);
        }
        if name.id == BuiltinId::ToString
            && let Some(text) = self.user_fmt_text(recv, "Display::fmt")?
        {
            return Ok(Some(Value::str(text)));
        }
        if matches!(recv, Value::IntW(..) | Value::F32(_))
            && matches!(name.id, BuiltinId::ToString | BuiltinId::Clone)
        {
            return Ok(Some(match name.id {
                BuiltinId::ToString => Value::str(recv.display()),
                _ => recv.clone(),
            }));
        }
        Ok(None)
    }

    pub(super) fn eval_method(
        self: &Arc<Self>,
        recv: &Value,
        name: &MethodName,
        args: &mut [Value],
    ) -> Result<Value> {
        let dereferenced = match recv {
            Value::Ref(reference) => match deref_receiver(reference, name, args)? {
                RefRead::Value(value) => Some(value),
                RefRead::StrGrown => return Ok(Value::Unit),
            },
            _ => None,
        };
        let recv = dereferenced.as_ref().unwrap_or(recv);
        if let Some(v) = self.pre_dispatch(recv, name, args)? {
            return Ok(v);
        }
        // The serde type tests and the pointer lookup apply to any receiver,
        // so they are answered before the per type dispatch below, which
        // returns early and would never reach them. Before `bridge_image` too,
        // since a u64 past `i64::MAX` saturates there and would then claim to
        // be an i64.
        if let Some(v) = methods::json_type_test(recv, &name.text) {
            return Ok(v);
        }
        if let Some(v) = methods::json_value_method(recv, &name.text, &*args) {
            return Ok(v);
        }
        // Integer methods answer from the real width, before `bridge_image`
        // below flattens the receiver to an i64 that saturates at `i64::MAX`
        // and forgets whether it was a u8 or a u64.
        if let Some(result) = int_method(recv, &name.text, args) {
            return result;
        }
        // f32 methods likewise: computed in real f32 before the image below
        // widens the receiver to an f64 that prints the wrong shortest form.
        if let Value::F32(f) = recv
            && let Some(value) = f32_method(*f, &name.text, args)?
        {
            return Ok(value);
        }
        let widened;
        let recv = match recv.bridge_image() {
            Some(image) => {
                widened = image;
                &widened
            }
            None => recv,
        };
        // Option and Result methods hand arguments through to the caller,
        // `unwrap_or` for one, and `flag.then_some(x)` on a bool does the
        // same, so their width tags must survive. `fold` hands its initial
        // value through the closure and the result the same way, and the
        // containers and a map entry store their arguments, so a pushed or
        // inserted number keeps its real width too.
        let hands_args_through = matches!(
            recv,
            Value::Enum { .. } | Value::Bool(_) | Value::Vec(_) | Value::Map(..)
        ) || matches!(recv, Value::Struct(st) if &**st.name() == "Entry")
            || name.id == BuiltinId::Fold;
        if !hands_args_through {
            for arg in args.iter_mut() {
                if let Some(image) = arg.bridge_image() {
                    *arg = image;
                }
            }
        }
        // The async http client, request, and response types.
        if let Some(res) = super::http::http_method(recv, &name.text, args) {
            return res;
        }
        if let Some(v) = range_builtin(recv, name, args)? {
            return Ok(v);
        }
        // A method on a range acts on its iterator value, and so does an
        // adaptor chain on a user type with its own `Iterator` impl, unless
        // the call is the user type's own method.
        let expanded;
        let recv = if matches!(recv, Value::Range { .. })
            || (self.has_user_next(recv) && !self.user_method_exists(recv, &name.text))
        {
            expanded = self.iterator_value(recv.clone())?;
            &expanded
        } else {
            recv
        };
        if let Value::Native(iterator) = recv
            && matches!(&*iterator.lock(), Native::Iterator(_))
            && let Some(value) = self.iterator_method(iterator, name, args)?
        {
            return Ok(value);
        }
        // A user defined `impl` method takes priority on a struct or enum, so a
        // script's own method is not shadowed by a builtin of the same name.
        let type_key = match recv {
            Value::Struct(st) => Some(st.name().to_string()),
            Value::Enum { enum_name, .. } => Some(enum_name.to_string()),
            _ => None,
        };
        if let Some(tk) = &type_key
            && let Some(chunk) = self.user_method(tk, &name.text)
        {
            let mut full = Vec::with_capacity(args.len() + 1);
            full.push(recv.clone());
            full.extend(args.iter().cloned());
            return self.run_chunk(&chunk, &full, &[]);
        }
        if name.id.is_higher_order()
            && let Some(v) = self.higher_order(recv, &name.text, &*args)?
        {
            return Ok(v);
        }
        // Vec::extend takes any IntoIterator, so a lazy argument such as
        // `.iter().map(..)` has to be drained here, where the interpreter is
        // in reach. The vec method itself cannot read one.
        if matches!(recv, Value::Vec(_))
            && matches!(name.text.as_str(), "extend" | "extend_from_slice")
            && let Some(first) = args.first()
            && !matches!(first, Value::Vec(_))
        {
            let items = self.drain_items(first.clone())?;
            args[0] = Value::vec(items);
        }
        Self::method_by_receiver(recv, name, args)
    }

    /// The per-receiver dispatch, after the any-receiver families above.
    fn method_by_receiver(recv: &Value, name: &MethodName, args: &mut [Value]) -> Result<Value> {
        let m = name.text.as_str();
        match recv {
            Value::Str(s) => methods::str_method(s, name, args),
            Value::Vec(v) => super::vecmap::vec_method(v, name, args),
            Value::Map(map, kind) => super::vecmap::map_method(map, *kind, name, args),
            Value::Enum { enum_name, .. } if &**enum_name == "Option" => {
                methods::opt_method(recv, name, args)
            }
            Value::Enum { enum_name, .. } if &**enum_name == "Result" => {
                methods::res_method(recv, name, args)
            }
            Value::Enum { .. } => methods::generic_method(recv, m, args),
            Value::Struct(st) if super::ratatui::is_ratatui_struct(st.name()) => {
                super::ratatui::struct_method(st, m, args)
            }
            Value::Struct(st) => match &**st.name() {
                "Command" => super::process::command_method(recv, m, args),
                "Child" => super::process::child_method(recv, m, args),
                "ExitStatus" => exitstatus_method(st, m),
                "Output" => output_method(st, m),
                "Duration" => duration_method(st, m, args),
                "DateTime" => datetime_method(st, m, args),
                "Entry" => methods::entry_method(st, m, args),
                "Path" | "PathBuf" => super::std_bridge::path_method(st, m, args),
                "OsString" => super::std_bridge::os_string_method(st, m),
                "DirEntry" => super::std_bridge::dir_entry_method(st, m),
                "FileType" => super::std_bridge::file_type_method(st, m),
                "Metadata" => super::std_bridge::metadata_method(st, m),
                "StdStream" => super::std_bridge::std_stream_method(st, m, args),
                "OpenOptions" => super::std_bridge::openoptions_method(st, m, args),
                "Permissions" => match m {
                    "mode" => Ok(st.get("mode").unwrap_or(Value::Int(0))),
                    "readonly" => Ok(st.get("readonly").unwrap_or(Value::Bool(false))),
                    "set_readonly" => Ok(Value::Unit),
                    _ => bail!("unknown method `{m}` on Permissions"),
                },
                "Rng" => super::crates_bridge::rng_method(m, args),
                "Base64Engine" => super::crates_bridge::base64_method(st, m, args),
                "Element" => super::xmltree_bridge::element_method(st, m, args),
                "RegKey" => super::winreg_bridge::winreg_method(st, m, args),
                "ServiceManager" => super::service_bridge::manager_method(st, m, args),
                "Service" => super::service_bridge::service_method(st, m, args),
                "WmiConnection" => super::wmi_bridge::wmi_method(st, m, args),
                _ => methods::generic_method(recv, m, args),
            },
            Value::Native(native) => Self::native_method(native, name, args),
            Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Char(_) => {
                scalar_method(recv, m, args)
            }
            other => methods::generic_method(other, m, args),
        }
    }

    fn native_method(
        native: &Arc<Mutex<Native>>,
        name: &MethodName,
        args: &mut [Value],
    ) -> Result<Value> {
        let m = name.text.as_str();
        if let Native::Instant(instant) = &*native.lock()
            && m == "elapsed"
        {
            return Ok(super::std_bridge::make_duration(instant.elapsed()));
        }
        // Files, readers, writers, sockets, children, clocks, temp files.
        if let Some(v) = super::native_methods::native_method(native, m, args)? {
            return Ok(v);
        }
        if let Some(v) = super::regex_bridge::regex_native_method(native, m, args)? {
            return Ok(v);
        }
        methods::generic_method(&Value::Native(native.clone()), m, args)
    }
}

// -- free helpers ----------------------------------------------------------

/// A `Type::CONST` path value: env consts, numeric limits, and the bridge
/// constants that hang off a type name. The widths that tag their values,
/// `u16::MAX`, carry the tag so the constant keeps its real width.
fn typed_path_constant(ty: &str, name: &str) -> Option<Value> {
    // `ErrorKind::NotFound` and friends compare against `e.kind()` answers.
    if ty == "ErrorKind" {
        return Some(Value::enum_of("ErrorKind", name, Vec::new()));
    }
    if ty == "consts" {
        let text = match name {
            "OS" => std::env::consts::OS,
            "ARCH" => std::env::consts::ARCH,
            "FAMILY" => std::env::consts::FAMILY,
            "EXE_EXTENSION" => std::env::consts::EXE_EXTENSION,
            "EXE_SUFFIX" => std::env::consts::EXE_SUFFIX,
            "PI" => return Some(Value::Float(PI)),
            _ => return None,
        };
        return Some(Value::str(text));
    }
    if let Some(v) = int_limit(ty, name) {
        return Some(v);
    }
    if let Some(v) = super::ratatui::ratatui_const(ty, name) {
        return Some(v);
    }
    if let Some(v) = super::service_bridge::service_variant(ty, name) {
        return Some(v);
    }
    if let Some(v) = super::jwt_bridge::jwt_algorithm(ty, name) {
        return Some(v);
    }
    if ty == "Ordering" {
        use std::cmp::Ordering::{Equal, Greater, Less};
        let o = match name {
            "Less" => Less,
            "Greater" => Greater,
            "Equal" => Equal,
            _ => return None,
        };
        return Some(make_ordering(o));
    }
    // A json null is None here, the same mapping the parser uses, so
    // `serde_json::Value::Null` written in a script lands on the same value.
    if ty == "Value" && name == "Null" {
        return Some(Value::none());
    }
    None
}

fn one(args: Vec<Value>) -> Result<Value> {
    args.into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("expected one argument"))
}

fn arg0(args: &[Value]) -> Value {
    args.first().cloned().unwrap_or(Value::Unit)
}

/// The builtin methods a range answers directly, before it expands to its
/// iterator value.
fn range_builtin(recv: &Value, name: &MethodName, args: &[Value]) -> Result<Option<Value>> {
    let Value::Range {
        start,
        end,
        inclusive,
    } = recv
    else {
        return Ok(None);
    };
    match name.id {
        BuiltinId::Clone => return Ok(Some(recv.clone())),
        BuiltinId::Contains => {
            let Some(Value::Int(value)) = args.first() else {
                bail!("range contains needs an integer");
            };
            return Ok(Some(Value::Bool(if *inclusive {
                *value >= *start && *value <= *end
            } else {
                *value >= *start && *value < *end
            })));
        }
        BuiltinId::Len | BuiltinId::Count => {
            let extra = i64::from(*inclusive && end >= start);
            return Ok(Some(Value::Int(end.saturating_sub(*start) + extra)));
        }
        BuiltinId::IsEmpty => {
            return Ok(Some(Value::Bool(if *inclusive {
                start > end
            } else {
                start >= end
            })));
        }
        _ => {}
    }
    Ok(None)
}

// `usize::MAX`, `i32::MIN`, `f32::NAN` and friends, at their real width.
// Returns None for anything that is not a numeric limit path.
fn int_limit(ty: &str, name: &str) -> Option<Value> {
    // The float limits first, `f64::EPSILON` guards float comparisons.
    if ty == "f64" {
        let v = match name {
            "EPSILON" => f64::EPSILON,
            "MAX" => f64::MAX,
            "MIN" => f64::MIN,
            "MIN_POSITIVE" => f64::MIN_POSITIVE,
            "INFINITY" => f64::INFINITY,
            "NEG_INFINITY" => f64::NEG_INFINITY,
            "NAN" => f64::NAN,
            _ => return None,
        };
        return Some(Value::Float(v));
    }
    if ty == "f32" {
        let v = match name {
            "EPSILON" => f32::EPSILON,
            "MAX" => f32::MAX,
            "MIN" => f32::MIN,
            "MIN_POSITIVE" => f32::MIN_POSITIVE,
            "INFINITY" => f32::INFINITY,
            "NEG_INFINITY" => f32::NEG_INFINITY,
            "NAN" => f32::NAN,
            _ => return None,
        };
        return Some(Value::F32(v));
    }
    // The 128-bit bounds live in `Value::Big`, u128's as reinterpreted bits.
    if ty == "i128" {
        return match name {
            "MAX" => Some(Value::Big(i128::MAX, super::numeric::IntWidth::I128)),
            "MIN" => Some(Value::Big(i128::MIN, super::numeric::IntWidth::I128)),
            _ => None,
        };
    }
    if ty == "u128" {
        return match name {
            "MAX" => Some(Value::Big(
                u128::MAX.cast_signed(),
                super::numeric::IntWidth::U128,
            )),
            "MIN" => Some(Value::Big(0, super::numeric::IntWidth::U128)),
            _ => None,
        };
    }
    let w = super::numeric::IntWidth::parse(ty)?;
    let value = match name {
        "MAX" => w.max(),
        "MIN" => w.min(),
        _ => return None,
    };
    Some(Value::int_of_width(value, w))
}

fn exitstatus_method(s: &Arc<super::value::StructData>, m: &str) -> Result<Value> {
    let success = matches!(s.get("success"), Some(Value::Bool(true)));
    let code = match s.get("code") {
        Some(Value::Int(c)) => Some(c),
        _ => None,
    };
    match shared::exit_status_core(m, success, code) {
        Some(shared::ExitOut::Bool(b)) => Ok(Value::Bool(b)),
        Some(shared::ExitOut::OptInt(Some(c))) => Ok(Value::some(Value::Int(c))),
        Some(shared::ExitOut::OptInt(None)) => Ok(Value::none()),
        None => bail!("unknown method `{m}` on ExitStatus"),
    }
}

fn output_method(s: &Arc<super::value::StructData>, m: &str) -> Result<Value> {
    Ok(match m {
        "status" | "stdout" | "stderr" => s.get(m).unwrap_or(Value::Unit),
        _ => bail!("unknown method `{m}` on Output"),
    })
}

fn sleep_future(args: &[Value]) -> Value {
    let duration = args
        .first()
        .and_then(super::std_bridge::duration_from_value)
        .unwrap_or(Duration::ZERO);
    Native::Future(Box::pin(async move {
        tokio::time::sleep(duration).await;
        Value::Unit
    }))
    .wrap()
}

fn yield_future() -> Value {
    Native::Future(Box::pin(async {
        tokio::task::yield_now().await;
        Value::Unit
    }))
    .wrap()
}

/// Build a `DateTime` value carrying its zone.
fn datetime_value(secs: i64, nanos: u32, local: bool, offset: i32) -> Value {
    Value::struct_of(
        "DateTime",
        [
            ("secs".into(), Value::Int(secs)),
            ("nanos".into(), Value::Int(i64::from(nanos))),
            ("local".into(), Value::Bool(local)),
            ("offset".into(), Value::Int(i64::from(offset))),
        ],
    )
}

/// Build a `DateTime` value for `Utc::now()` / `Local::now()`,storing the
/// unix timestamp.
fn now_datetime(local: bool) -> Value {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    datetime_value(
        i64::try_from(now.as_secs()).unwrap_or(i64::MAX),
        now.subsec_nanos(),
        local,
        0,
    )
}

fn datetime_method(s: &Arc<super::value::StructData>, m: &str, args: &[Value]) -> Result<Value> {
    let secs = match s.get("secs") {
        Some(Value::Int(v)) => v,
        _ => 0,
    };
    let nanos = match s.get("nanos") {
        Some(Value::Int(v)) => u32::try_from(v).unwrap_or_default(),
        _ => 0,
    };
    let local = matches!(s.get("local"), Some(Value::Bool(true)));
    let offset = match s.get("offset") {
        Some(Value::Int(v)) => i32::try_from(v).unwrap_or_default(),
        _ => 0,
    };
    match shared::datetime_core(m, secs, nanos, local, offset, &VArgs(args)) {
        Some(shared::DateOut::Int(i)) => Ok(Value::Int(i)),
        Some(shared::DateOut::Text(t)) => Ok(Value::str(t)),
        None => bail!("unknown method `{m}` on DateTime"),
    }
}

fn duration_method(s: &Arc<super::value::StructData>, m: &str, args: &[Value]) -> Result<Value> {
    let secs = u64::try_from(super::std_bridge::field_int(s, "secs")).unwrap_or_default();
    let nanos = u32::try_from(super::std_bridge::field_int(s, "nanos")).unwrap_or_default();
    if let "checked_add" | "checked_sub" = m {
        let own = Duration::new(secs, nanos);
        let Some(other) = args
            .first()
            .and_then(super::std_bridge::duration_from_value)
        else {
            bail!("`{m}` on Duration takes a Duration argument");
        };
        let out = match m {
            "checked_add" => own.checked_add(other),
            _ => own.checked_sub(other),
        };
        return Ok(out.map_or_else(Value::none, |d| {
            Value::some(super::std_bridge::make_duration(d))
        }));
    }
    match shared::duration_core(m, secs, nanos) {
        Some(shared::DurOut::Int(i)) => Ok(Value::Int(i)),
        Some(shared::DurOut::Float(f)) => Ok(Value::Float(f)),
        Some(shared::DurOut::Bool(b)) => Ok(Value::Bool(b)),
        None => bail!("unknown method `{m}` on Duration"),
    }
}

/// Width-aware integer methods.
fn int_method(recv: &Value, m: &str, args: &[Value]) -> Option<Result<Value>> {
    let (value, mut width) = recv.int_parts()?;
    let mut decoded = Vec::with_capacity(args.len());
    for arg in args {
        let (arg_value, arg_width) = arg.int_parts()?;
        decoded.push(arg_value);
        // Receiver and argument share one type in real Rust, so a width
        // either side states answers for both. A shift amount's own u32 must not redefine the receiver.
        if !super::int_methods::takes_amount_arg(m)
            && let Ok(unified) = super::numeric::unify(width, arg_width)
        {
            width = unified;
        }
    }
    Some(
        match super::int_methods::int_method(m, width, value, &decoded)? {
            Ok(out) => Ok(int_out(out, width)),
            Err(error) => Err(error),
        },
    )
}

/// Materialize an f32 core answer as a runtime value. Called before
/// `bridge_image` widens the receiver, so the result keeps the f32 tag.
fn f32_method(recv: f32, name: &str, args: &[Value]) -> Result<Option<Value>> {
    Ok(
        shared::f32_core(recv, name, &VArgs(args))?.map(|out| match out {
            F32Out::Val(value) => Value::F32(value),
            F32Out::Bool(value) => Value::Bool(value),
            F32Out::SomeOrdering(ordering) => Value::some(make_ordering(ordering)),
        }),
    )
}

fn int_out(out: super::int_methods::IntOut, width: super::numeric::IntWidth) -> Value {
    use super::int_methods::IntOut;
    match out {
        IntOut::Same(value) => Value::int_of_width(value, width),
        // The counting family answers u32 in real Rust, so the tag has to say
        // so, or `!x.count_ones()` computes in 64 bits and prints -1 where the
        // compiled binary prints 4294967295.
        IntOut::Count(count) => {
            Value::int_of_width(i128::from(count), super::numeric::IntWidth::U32)
        }
        IntOut::Bool(value) => Value::Bool(value),
        IntOut::Checked(Some(value)) => Value::some(Value::int_of_width(value, width)),
        IntOut::Checked(None) => Value::none(),
        IntOut::SomeFloat(value) => Value::some(Value::Float(value)),
        IntOut::Ordering(ordering) => make_ordering(ordering),
        IntOut::Bytes(bytes) => Value::vec(
            bytes
                .into_iter()
                .map(|byte| Value::Int(i64::from(byte)))
                .collect(),
        ),
    }
}

fn scalar_method(recv: &Value, m: &str, args: &[Value]) -> Result<Value> {
    // A conversion that only changes the static type is a no-op on a scalar,
    // and a number never reaches a generic dispatch, so these are answered
    // here. `2.into()` for a `serde_json::Number` is the same 2.
    match m {
        "to_string" => return Ok(Value::str(recv.display())),
        "clone" | "into" => return Ok(recv.clone()),
        _ => {}
    }
    // Serde accessors on an already decoded scalar. A json bool arrives as a
    // plain Bool here, so `as_bool` has to answer on it, and an accessor for
    // the wrong type is None rather than an error, matching serde.
    if matches!(
        m,
        "as_str"
            | "as_i64"
            | "as_u64"
            | "as_f64"
            | "as_bool"
            | "as_array"
            | "as_array_mut"
            | "as_object"
            | "as_object_mut"
    ) {
        let matched = match (recv, m) {
            (Value::Bool(_), "as_bool")
            | (Value::Str(_), "as_str")
            | (Value::Int(_) | Value::IntW(..), "as_i64" | "as_u64")
            | (Value::Float(_), "as_f64") => true,
            (Value::Int(i), "as_f64") => {
                return Ok(Value::some(Value::Float(AsPrimitive::<f64>::as_(*i))));
            }
            _ => false,
        };
        return Ok(if matched {
            Value::some(recv.clone())
        } else {
            Value::none()
        });
    }
    let n = match recv {
        Value::Int(i) => Some(Num::Int(*i)),
        Value::Float(f) => Some(Num::Float(*f)),
        _ => None,
    };
    if let Some(n) = n {
        if let Some(out) = shared::num_core(n, m, &VArgs(args))? {
            return Ok(num_out(out));
        }
        bail!("unknown method `{m}` on a number");
    }
    if let Value::Char(ch) = recv
        && let Some(out) = shared::char_method(*ch, m, &VArgs(args))
    {
        return Ok(match out? {
            CharOut::Bool(v) => Value::Bool(v),
            CharOut::Char(c) => Value::Char(c),
            CharOut::Str(s) => Value::str(s),
            CharOut::OptU32(Some(digit)) => Value::some(Value::int_of_width(
                i128::from(digit),
                super::numeric::IntWidth::U32,
            )),
            CharOut::OptU32(None) => Value::none(),
        });
    }
    methods::generic_method(recv, m, args)
}

/// Turn a neutral numeric core answer into a runtime value.
fn num_out(out: NumOut) -> Value {
    match out {
        NumOut::Int(i) => Value::Int(i),
        NumOut::Float(f) => Value::Float(f),
        NumOut::Bool(b) => Value::Bool(b),
        NumOut::SomeInt(i) => Value::some(Value::Int(i)),
        NumOut::SomeFloat(f) => Value::some(Value::Float(f)),
        NumOut::Nothing => Value::none(),
        NumOut::Ordering(o) => make_ordering(o),
        NumOut::SomeOrdering(o) => Value::some(make_ordering(o)),
    }
}

/// A closure that forwards its arguments to a path call, so a path written as
/// a value can be handed to `map` or `and_then`.
fn path_closure(segs: Vec<String>, num_params: usize) -> Value {
    Value::Closure(Arc::new(ClosureData {
        chunk: super::bytecode::path_call_chunk(segs, num_params),
        captured: Vec::new(),
    }))
}

/// The interpreter's argument view for the shared cores.
pub(super) struct VArgs<'a>(pub(super) &'a [Value]);

impl Args for VArgs<'_> {
    fn text(&self, i: usize) -> String {
        self.0.get(i).map(Value::display).unwrap_or_default()
    }

    fn int(&self, i: usize) -> Option<i64> {
        match self.0.get(i) {
            Some(Value::Int(n)) => Some(*n),
            Some(tagged @ Value::IntW(..)) => tagged.untag_int(),
            _ => None,
        }
    }

    fn float(&self, i: usize) -> Option<f64> {
        match self.0.get(i) {
            Some(Value::Float(f)) => Some(*f),
            Some(Value::F32(f)) => Some(f64::from(*f)),
            Some(Value::Int(n)) => Some(AsPrimitive::<f64>::as_(*n)),
            Some(tagged @ Value::IntW(..)) => tagged.untag_int().map(AsPrimitive::<f64>::as_),
            _ => None,
        }
    }

    fn pattern_chars(&self, i: usize) -> Option<Vec<char>> {
        let Some(Value::Vec(items)) = self.0.get(i) else {
            return None;
        };
        Some(
            items
                .lock()
                .iter()
                .filter_map(|v| match v {
                    Value::Char(c) => Some(*c),
                    Value::Str(text) => text.chars().next(),
                    _ => None,
                })
                .collect(),
        )
    }
}

/// What reading a method receiver through a reference produced.
enum RefRead {
    Value(Value),
    /// A string grow method already ran and stored back through the
    /// reference, nothing further to dispatch.
    StrGrown,
}

/// Read a method's receiver through its reference. A mutating method splits
/// the referenced slot from value sharing first, so the in-place mutation
/// stays private to the borrowed place. A string mutates by growing its own
/// buffer rather than shared storage, so the grown buffer stores back
/// through the reference to land in the borrowed place.
fn deref_receiver(
    reference: &super::value::ValueRef,
    name: &MethodName,
    args: &[Value],
) -> Result<RefRead> {
    let read = if super::bytecode::builtin_mutating(&name.text) {
        reference.get_unique()
    } else {
        reference.get()
    };
    let Some(value) = read else {
        bail!("method call through a dangling reference");
    };
    if let Value::Str(s) = &value
        && matches!(name.id, BuiltinId::Push | BuiltinId::PushStr)
    {
        let mut grown = s.clone();
        match (&name.id, args.first()) {
            (BuiltinId::Push, Some(Value::Char(c))) => grown.push(*c),
            (BuiltinId::PushStr, Some(Value::Str(other))) => grown.push_str(other),
            (BuiltinId::PushStr, Some(other)) => grown.push_str(&other.display()),
            _ => {}
        }
        reference.set(Value::Str(grown));
        return Ok(RefRead::StrGrown);
    }
    Ok(RefRead::Value(value))
}

// -- template rendering ----------------------------------------------------

fn render_template(
    vm: &Arc<Vm>,
    template: &str,
    positional: &[Value],
    named: &[(&str, Value)],
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
                        Value::Int(i) => Ok(i),
                        ref other @ Value::IntW(..) => other
                            .untag_int()
                            .ok_or_else(|| anyhow::anyhow!("format width out of range")),
                        other => {
                            bail!("format width must be an integer, got {}", other.type_name())
                        }
                    }
                };
                let fmt = super::format::expand_widths_with(fmt, &mut lookup)?;
                let number = match &value {
                    Value::Float(f) => Some(super::format::SpecNumber::Float(*f)),
                    Value::F32(f) => Some(super::format::SpecNumber::F32(*f)),
                    Value::Int(i) => Some(super::format::SpecNumber::Int(*i)),
                    Value::IntW(v, w) => Some(super::format::SpecNumber::Sized {
                        value: w.decode(*v),
                        bits: w.bits(),
                    }),
                    _ => None,
                };
                // A user `Display` or `Debug` impl overrides the built-in
                // rendering. Only the form the spec asks for runs, an impl
                // may have side effects.
                let wants_debug = fmt.contains('?');
                let display_text = if wants_debug {
                    String::new()
                } else {
                    match vm.user_fmt_text(&value, "Display::fmt")? {
                        Some(text) => text,
                        None => value.display(),
                    }
                };
                let debug_text = if wants_debug {
                    match vm.user_fmt_text(&value, "Debug::fmt")? {
                        Some(text) => text,
                        None => value.debug(),
                    }
                } else {
                    String::new()
                };
                out.push_str(&super::format::apply_spec(
                    &fmt,
                    &display_text,
                    &debug_text,
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
    positional: &[Value],
    named: &[(&str, Value)],
) -> Result<Value> {
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
