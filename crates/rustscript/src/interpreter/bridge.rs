//! The bridge front door: format rendering, method and path
//! dispatch. The per-receiver method families live in `methods`, `vecmap`,
//! `higher_order`, and `iterator`; this file routes to them.

use num_traits::AsPrimitive;
use std::f64::consts::PI;
use std::mem::take;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use parking_lot::Mutex;

use super::bytecode::Chunk;
use super::bytecode::{BuiltinId, BuiltinId as B, MethodName, PathId, PathRef};
use super::enum_def::EnumKind;
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
    pub(super) fn user_fmt_text(
        self: &Arc<Self>,
        v: &Value,
        debug: bool,
    ) -> Result<Option<String>> {
        let Some(methods) = self.impls.of_value(v) else {
            return Ok(None);
        };
        let Some(chunk) = (if debug {
            &methods.debug
        } else {
            &methods.display
        })
        .clone() else {
            return Ok(None);
        };
        let handle = Arc::new(parking_lot::Mutex::new(Native::Fmt(String::new())));
        let args = vec![v.clone(), Value::Native(handle.clone())];
        self.run_chunk(&chunk, &args, &[])?;
        let text = match &*handle.lock() {
            Native::Fmt(s) => s.clone(),
            _ => String::new(),
        };
        Ok(Some(text))
    }

    /// Run the value's user `Drop::drop` when this is its last holder, then
    /// drop what it contains. Another holder means the value was moved or is
    /// still shared, and the real owner drops it at its own end of life.
    /// Containers, cells, and `Rc` hand their contents on when they die, so
    /// a guard inside them still drops. A shared cycle never reaches one
    /// holder, so it leaks exactly like real `Rc` cycles do.
    pub(super) fn run_user_drop(self: &Arc<Self>, value: Value) -> Result<()> {
        match value {
            Value::Struct(s) => {
                if Arc::strong_count(&s) != 1 {
                    return Ok(());
                }
                self.run_drop_impl(Value::Struct(s.clone()))?;
                // The impl could have stored a clone of self somewhere, in
                // which case the fields live on with it.
                if Arc::strong_count(&s) != 1 {
                    return Ok(());
                }
                // Fields drop after the type's own `Drop::drop`, in
                // declaration order, like real Rust.
                let fields = take(&mut *s.values.lock());
                for field in fields {
                    self.run_user_drop(field)?;
                }
                Ok(())
            }
            Value::Enum { def, variant, data } => {
                if Arc::strong_count(&data) != 1 {
                    return Ok(());
                }
                self.run_drop_impl(Value::Enum {
                    def,
                    variant,
                    data: data.clone(),
                })?;
                if Arc::strong_count(&data) != 1 {
                    return Ok(());
                }
                let payload = take(&mut *data.lock());
                for field in payload {
                    self.run_user_drop(field)?;
                }
                Ok(())
            }
            Value::Vec(list) | Value::Tuple(list) => {
                if Arc::strong_count(&list) != 1 {
                    return Ok(());
                }
                let items = take(&mut *list.lock());
                for item in items {
                    self.run_user_drop(item)?;
                }
                Ok(())
            }
            Value::Map(map, _) => {
                if Arc::strong_count(&map) != 1 {
                    return Ok(());
                }
                let entries = take(&mut *map.lock());
                for (_, entry) in entries {
                    self.run_user_drop(entry)?;
                }
                Ok(())
            }
            Value::Cell(_, slot) => {
                if Arc::strong_count(&slot) != 1 {
                    return Ok(());
                }
                let inner = take(&mut *slot.lock());
                self.run_user_drop(inner)
            }
            _ => Ok(()),
        }
    }

    /// Run the value type's own `Drop::drop`, when the script defines one.
    fn run_drop_impl(self: &Arc<Self>, value: Value) -> Result<()> {
        let Some(chunk) = self
            .impls
            .of_value(&value)
            .and_then(|methods| methods.drop.clone())
        else {
            return Ok(());
        };
        self.run_chunk(&chunk, &[value], &[])?;
        Ok(())
    }

    // -- path values -------------------------------------------------------

    pub(super) fn eval_path_value(&self, path: &PathRef) -> Result<Value> {
        if path.id == PathId::Other {
            return self.user_path_value(path);
        }
        if let Some(v) = path_constant(path.id) {
            return Ok(v);
        }
        // A path used as a function value. A zero-arg constructor like
        // `Vec::new` handed to `or_insert_with` becomes a nullary closure.
        // Anything else, `Some` handed to `map`, becomes a one-arg closure,
        // and `dispatch_call` runs the call.
        let arity = usize::from(!matches!(path.id.name(), "new" | "default"));
        Ok(path_closure(path.clone(), arity))
    }

    /// A user item used as a value: a unit variant, a unit struct, a
    /// function by bare name, or a method reference.
    fn user_path_value(&self, path: &PathRef) -> Result<Value> {
        let segs = &path.segs;
        let Some(last) = segs.last().map(String::as_str) else {
            bail!("empty path");
        };
        if segs.len() >= 2 {
            let ty = segs[segs.len() - 2].as_str();
            if let Some(v) = self.unit_variant(Some(ty), last) {
                return Ok(v);
            }
        } else {
            if let Some(v) = self.unit_variant(None, last) {
                return Ok(v);
            }
            // A unit struct used as a value, `struct Marker;` then
            // `Marker`.
            if let Some(name) = self
                .unit_structs
                .iter()
                .find(|name| &***name == last || super::resolver::bare(name) == last)
            {
                let type_id = self.impls.type_id(name);
                return Ok(Value::structure(
                    super::value::StructShape::typed(&**name, type_id, Vec::new(), Vec::new()),
                    Vec::new(),
                ));
            }
            // A bare function name used as a value, `.map(strip_html)`. The
            // closure forwards its arguments to the call, which
            // `dispatch_call` resolves back to the user function.
            if let Some(chunk) = self.user_function(last) {
                return Ok(path_closure(path.clone(), chunk.num_params));
            }
        }
        // A path used as a function value. A zero-arg constructor handed to
        // `or_insert_with` becomes a nullary closure. A method reference like
        // `Value::as_str` handed to `and_then` becomes a one-arg closure, and
        // `dispatch_call` resolves it as a UFCS method call on that argument.
        if matches!(last, "new" | "default") {
            return Ok(path_closure(path.clone(), 0));
        }
        if segs.len() >= 2
            && let Some(chunk) = self
                .user_method(&segs[segs.len() - 2], last)
                .or_else(|| self.user_function(&path.display()))
                .or_else(|| self.user_function(last))
        {
            return Ok(path_closure(path.clone(), chunk.num_params));
        }
        // A SCREAMING_CASE tail is a constant, never a function, so
        // wrapping it in the closure fallback would smuggle a closure
        // value into arithmetic.
        if last.chars().any(|c| c.is_ascii_uppercase())
            && !last.chars().any(|c| c.is_ascii_lowercase())
        {
            bail!("unsupported constant `{}`", path.display());
        }
        Ok(path_closure(path.clone(), 1))
    }

    // -- path calls --------------------------------------------------------

    pub(super) fn dispatch_call(
        self: &Arc<Self>,
        path: &PathRef,
        args: Vec<Value>,
    ) -> Result<Value> {
        match path.id {
            PathId::Other => return self.dispatch_user_call(path, args),
            PathId::Some => return Ok(Value::some(one(args)?)),
            PathId::Ok => return Ok(Value::ok(one(args)?)),
            PathId::Err => return Ok(Value::err(one(args)?)),
            // A user type with a `Drop` impl runs it now, its register was
            // cleared at the call site so this is the last holder. Anything
            // else dies with its register, file writes are unbuffered.
            PathId::Drop => {
                self.run_user_drop(one(args)?)?;
                return Ok(Value::Unit);
            }
            // The Ctrl-C handler must reach back into the interpreter to run
            // the script's own closure, so it cannot go through the plain
            // native call.
            PathId::CtrlcSetHandler => {
                let closure = args.first().cloned().unwrap_or(Value::Unit);
                return Ok(match super::set_ctrlc_handler(closure) {
                    Ok(()) => Value::ok(Value::Unit),
                    Err(e) => Value::err(Value::str(e.to_string())),
                });
            }
            // sleep is the one thread function that needs no threading.
            PathId::ThreadSleep => {
                let Some(d) = args
                    .first()
                    .and_then(super::std_bridge::duration_from_value)
                else {
                    bail!("thread::sleep takes a Duration");
                };
                std::thread::sleep(d);
                return Ok(Value::Unit);
            }
            // `tokio::sync::Mutex` is its own kind: its `lock` is awaited and
            // answers the guard with no `Result` layer, unlike `std::sync`.
            PathId::TokioSyncMutexNew => {
                let inner = args.into_iter().next().unwrap_or(Value::Unit);
                return Ok(super::cell::make_cell(
                    super::value::CellKind::TokioMutex,
                    inner,
                ));
            }
            // A json value written out in a script, `Value::String(s)`. A
            // parsed json is held as native values here, so each variant is
            // exactly its own payload.
            PathId::ValueString
            | PathId::ValueBool
            | PathId::ValueNumber
            | PathId::ValueArray
            | PathId::ValueObject => return one(args),
            _ => {}
        }
        // Namespaced calls reaching native bridges compute in i64 and f64, so
        // width-tagged numbers pass those their plain image. The script-level
        // targets above and in `dispatch_user_call` hand values through, so
        // they keep the real args and their width tags.
        let images: Vec<Value> = args
            .iter()
            .map(|arg| match arg.bridge_image() {
                Some(image) => image,
                None => arg.clone(),
            })
            .collect();
        match bridge_call(path.id, &images)? {
            Some(v) => Ok(v),
            None => bail!("unsupported call `{}`", path.display()),
        }
    }

    /// A call the path table does not name: a script function, a method on
    /// a user type, a tuple struct or variant, or a UFCS method reference.
    fn dispatch_user_call(self: &Arc<Self>, path: &PathRef, args: Vec<Value>) -> Result<Value> {
        let [.., namespace, last] = path.segs.as_slice() else {
            let name = path.segs.first().map_or("", String::as_str);
            if let Some(chunk) = self.user_function(name) {
                return self.run_chunk(&chunk, &args, &[]);
            }
            if self.struct_names.contains(name) {
                return Ok(self.make_tuple_struct(name, args));
            }
            if let Some(v) = self.make_tuple_variant(None, name, &args) {
                return Ok(v);
            }
            bail!("unknown function `{name}`");
        };
        if namespace == "thread" {
            bail!("std::thread is not supported beyond sleep, use tokio::spawn");
        }
        if let Some(chunk) = self.user_function(&path.display()) {
            return self.run_chunk(&chunk, &args, &[]);
        }
        // A method on a user type, `Type::assoc(..)` or UFCS
        // `Type::method(recv, ..)`. The receiver, if any, is simply
        // the first argument, matching param 0.
        if let Some(chunk) = self.user_method(namespace, last) {
            return self.run_chunk(&chunk, &args, &[]);
        }
        if let Some(v) = self.make_tuple_variant(Some(namespace), last, &args) {
            return Ok(v);
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
            let name = self.impls.method_name(last);
            return self.eval_method(&recv, &name, &mut rest);
        }
        bail!("unsupported call `{}`", path.display())
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
            if let Some(v) = super::cell::cell_method(*kind, slot, name.id, args)? {
                return Ok(Some(v));
            }
            let inner = slot.lock().clone();
            return self.eval_method(&inner, name, args).map(Some);
        }
        if name.id == BuiltinId::ToString
            && let Some(text) = self.user_fmt_text(recv, false)?
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
        if let Some(v) = methods::json_type_test(recv, name) {
            return Ok(v);
        }
        if let Some(v) = methods::json_value_method(recv, name, &*args) {
            return Ok(v);
        }
        // Integer methods answer from the real width, before `bridge_image`
        // below flattens the receiver to an i64 that saturates at `i64::MAX`
        // and forgets whether it was a u8 or a u64.
        if let Some(result) = int_method(recv, name, args) {
            return result;
        }
        // f32 methods likewise: computed in real f32 before the image below
        // widens the receiver to an f64 that prints the wrong shortest form.
        if let Value::F32(f) = recv
            && let Some(value) = f32_method(*f, name.id, args)?
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
        ) || matches!(recv, Value::Native(n) if matches!(&*n.lock(), Native::Entry { .. }))
            || name.id == BuiltinId::Fold;
        if !hands_args_through {
            for arg in args.iter_mut() {
                if let Some(image) = arg.bridge_image() {
                    *arg = image;
                }
            }
        }
        // The async http client, request, and response types.
        if let Some(res) = super::http::http_method(recv, name, args) {
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
            || (self.has_user_next(recv) && !self.user_method_exists(recv, name))
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
        if let Some(methods) = self.impls.of_value(recv)
            && let Some(chunk) = methods.get(name)
        {
            let chunk = chunk.clone();
            let mut full = Vec::with_capacity(args.len() + 1);
            full.push(recv.clone());
            full.extend(args.iter().cloned());
            return self.run_chunk(&chunk, &full, &[]);
        }
        if name.id.is_higher_order()
            && let Some(v) = self.higher_order(recv, name.id, &*args)?
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
            Value::Enum { def, .. } if def.kind == EnumKind::Option => {
                methods::opt_method(recv, name, args)
            }
            Value::Enum { def, .. } if def.kind == EnumKind::Result => {
                methods::res_method(recv, name, args)
            }
            Value::Enum { .. } => methods::generic_method(recv, name, args),
            Value::Struct(st) if super::ratatui::is_ratatui_struct(st.name()) => {
                super::ratatui::struct_method(st, name, args)
            }
            Value::Struct(st) => match &**st.name() {
                "Command" => super::process::command_method(recv, name, args),
                "Child" => super::process::child_method(recv, name, args),
                "ExitStatus" => exitstatus_method(st, name),
                "Output" => output_method(st, name),
                "Duration" => duration_method(st, name, args),
                "DateTime" => datetime_method(st, name, args),
                "Path" | "PathBuf" => super::std_bridge::path_method(st, name, args),
                "OsString" => super::std_bridge::os_string_method(st, name),
                "DirEntry" => super::std_bridge::dir_entry_method(st, name),
                "FileType" => super::std_bridge::file_type_method(st, name),
                "Metadata" => super::std_bridge::metadata_method(st, name),
                "StdStream" => super::std_bridge::std_stream_method(st, name, args),
                "OpenOptions" => super::std_bridge::openoptions_method(st, name, args),
                "Permissions" => match m {
                    "mode" => Ok(st.get("mode").unwrap_or(Value::Int(0))),
                    "readonly" => Ok(st.get("readonly").unwrap_or(Value::Bool(false))),
                    "set_readonly" => Ok(Value::Unit),
                    _ => bail!("unknown method `{m}` on Permissions"),
                },
                "Rng" => super::crates_bridge::rng_method(name, args),
                "Base64Engine" => super::crates_bridge::base64_method(st, name, args),
                "Element" => super::xmltree_bridge::element_method(st, name, args),
                "RegKey" => super::winreg_bridge::winreg_method(st, name, args),
                "ServiceManager" => super::service_bridge::manager_method(st, name, args),
                "Service" => super::service_bridge::service_method(st, name, args),
                "WmiConnection" => super::wmi_bridge::wmi_method(st, name, args),
                _ => methods::generic_method(recv, name, args),
            },
            Value::Native(native) => Self::native_method(native, name, args),
            Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Char(_) => {
                scalar_method(recv, name, args)
            }
            other => methods::generic_method(other, name, args),
        }
    }

    fn native_method(
        native: &Arc<Mutex<Native>>,
        name: &MethodName,
        args: &mut [Value],
    ) -> Result<Value> {
        let m = name.text.as_str();
        let entry = match &*native.lock() {
            Native::Entry { map, key } => Some((map.clone(), key.clone())),
            _ => None,
        };
        if let Some((map, key)) = entry {
            return methods::entry_method(&map, &key, name, args);
        }
        if let Native::Instant(instant) = &*native.lock()
            && m == "elapsed"
        {
            return Ok(super::std_bridge::make_duration(instant.elapsed()));
        }
        // Files, readers, writers, sockets, children, clocks, temp files.
        if let Some(v) = super::native_methods::native_method(native, name, args)? {
            return Ok(v);
        }
        if let Some(v) = super::regex_bridge::regex_native_method(native, name, args)? {
            return Ok(v);
        }
        methods::generic_method(&Value::Native(native.clone()), name, args)
    }
}

// -- free helpers ----------------------------------------------------------

/// A path value the table names: env consts, numeric limits, and the bridge
/// constants that hang off a type name. None for a path that names a
/// function, which becomes a closure instead.
fn path_constant(id: PathId) -> Option<Value> {
    let text = match id {
        PathId::UnixEpoch => return Some(Native::SystemTime(std::time::UNIX_EPOCH).wrap()),
        // A json null is None here, the same mapping the parser uses, so
        // `serde_json::Value::Null` written in a script lands on the same
        // value.
        PathId::ValueNull => return Some(Value::none()),
        PathId::ConstsPi => return Some(Value::Float(PI)),
        PathId::ConstsOs => std::env::consts::OS,
        PathId::ConstsArch => std::env::consts::ARCH,
        PathId::ConstsFamily => std::env::consts::FAMILY,
        PathId::ConstsExeExtension => std::env::consts::EXE_EXTENSION,
        PathId::ConstsExeSuffix => std::env::consts::EXE_SUFFIX,
        _ => {
            return numeric_limit(id)
                .or_else(|| super::crates_bridge::base64_engine(id))
                .or_else(|| super::winreg_bridge::winreg_const(id))
                .or_else(|| super::service_bridge::service_const(id))
                .or_else(|| super::ratatui::ratatui_const(id));
        }
    };
    Some(Value::str(text))
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

/// `usize::MAX`, `i32::MIN`, `f32::NAN` and friends, at their real width.
/// The widths that tag their values, `u16::MAX`, carry the tag so the
/// constant keeps its real width.
fn numeric_limit(id: PathId) -> Option<Value> {
    use super::numeric::IntWidth;
    Some(match id {
        // The float limits first, `f64::EPSILON` guards float comparisons.
        PathId::F64Epsilon => Value::Float(f64::EPSILON),
        PathId::F64Max => Value::Float(f64::MAX),
        PathId::F64Min => Value::Float(f64::MIN),
        PathId::F64MinPositive => Value::Float(f64::MIN_POSITIVE),
        PathId::F64Infinity => Value::Float(f64::INFINITY),
        PathId::F64NegInfinity => Value::Float(f64::NEG_INFINITY),
        PathId::F64Nan => Value::Float(f64::NAN),
        PathId::F32Epsilon => Value::F32(f32::EPSILON),
        PathId::F32Max => Value::F32(f32::MAX),
        PathId::F32Min => Value::F32(f32::MIN),
        PathId::F32MinPositive => Value::F32(f32::MIN_POSITIVE),
        PathId::F32Infinity => Value::F32(f32::INFINITY),
        PathId::F32NegInfinity => Value::F32(f32::NEG_INFINITY),
        PathId::F32Nan => Value::F32(f32::NAN),
        // The 128-bit bounds live in `Value::Big`, u128's as reinterpreted
        // bits.
        PathId::I128Max => Value::Big(i128::MAX, IntWidth::I128),
        PathId::I128Min => Value::Big(i128::MIN, IntWidth::I128),
        PathId::U128Max => Value::Big(u128::MAX.cast_signed(), IntWidth::U128),
        PathId::U128Min => Value::Big(0, IntWidth::U128),
        PathId::I8Max
        | PathId::I16Max
        | PathId::I32Max
        | PathId::I64Max
        | PathId::IsizeMax
        | PathId::U8Max
        | PathId::U16Max
        | PathId::U32Max
        | PathId::U64Max
        | PathId::UsizeMax => {
            let w = IntWidth::parse(id.namespace())?;
            Value::int_of_width(w.max(), w)
        }
        PathId::I8Min
        | PathId::I16Min
        | PathId::I32Min
        | PathId::I64Min
        | PathId::IsizeMin
        | PathId::U8Min
        | PathId::U16Min
        | PathId::U32Min
        | PathId::U64Min
        | PathId::UsizeMin => {
            let w = IntWidth::parse(id.namespace())?;
            Value::int_of_width(w.min(), w)
        }
        _ => return None,
    })
}

/// The bridge call behind a table path: chrono, the async sleeps, reqwest,
/// ratatui, std, and the associated functions. None when no bridge answers
/// the id, which is a table entry with no implementation behind it.
fn bridge_call(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    match id {
        PathId::UtcNow | PathId::LocalNow => {
            return Ok(Some(now_datetime(id == PathId::LocalNow)));
        }
        PathId::DateTimeParseFromRfc3339 => {
            return Ok(Some(match parse_rfc3339(&arg0(args).display()) {
                Ok((unix_secs, nanos, offset)) => {
                    Value::ok(datetime_value(unix_secs, nanos, false, offset))
                }
                Err(e) => Value::err(Value::str(e)),
            }));
        }
        PathId::TimeSleep => return Ok(Some(sleep_future(args))),
        PathId::TaskYieldNow => return Ok(Some(yield_future())),
        PathId::ReqwestGet
        | PathId::ReqwestBlockingGet
        | PathId::ReqwestClientNew
        | PathId::ReqwestClientBuilder
        | PathId::ReqwestBlockingClientNew
        | PathId::ReqwestBlockingClientBuilder
        | PathId::RedirectPolicyNone
        | PathId::RedirectPolicyLimited => return super::http::reqwest_call(id, args).map(Some),
        _ => {}
    }
    if let Some(v) = super::ratatui::ratatui_assoc(id, args) {
        return Ok(Some(v));
    }
    if let Some(v) = super::std_bridge::native_call(id, args)? {
        return Ok(Some(v));
    }
    super::assoc::assoc_fn(id, args)
}

fn exitstatus_method(s: &Arc<super::value::StructData>, name: &MethodName) -> Result<Value> {
    let m = name.id;
    let success = matches!(s.get("success"), Some(Value::Bool(true)));
    let code = match s.get("code") {
        Some(Value::Int(c)) => Some(c),
        _ => None,
    };
    match shared::exit_status_core(m, success, code) {
        Some(shared::ExitOut::Bool(b)) => Ok(Value::Bool(b)),
        Some(shared::ExitOut::OptInt(Some(c))) => Ok(Value::some(Value::Int(c))),
        Some(shared::ExitOut::OptInt(None)) => Ok(Value::none()),
        None => bail!("unknown method `{}` on ExitStatus", name.text),
    }
}

fn output_method(s: &Arc<super::value::StructData>, name: &MethodName) -> Result<Value> {
    let m = name.id;
    Ok(match m {
        B::Status | B::Stdout | B::Stderr => s.get(m.name()).unwrap_or(Value::Unit),
        _ => bail!("unknown method `{}` on Output", name.text),
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

fn datetime_method(
    s: &Arc<super::value::StructData>,
    name: &MethodName,
    args: &[Value],
) -> Result<Value> {
    let m = name.id;
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
        None => bail!("unknown method `{}` on DateTime", name.text),
    }
}

fn duration_method(
    s: &Arc<super::value::StructData>,
    name: &MethodName,
    args: &[Value],
) -> Result<Value> {
    let m = name.id;
    let secs = u64::try_from(super::std_bridge::field_int(s, "secs")).unwrap_or_default();
    let nanos = u32::try_from(super::std_bridge::field_int(s, "nanos")).unwrap_or_default();
    if let B::CheckedAdd | B::CheckedSub = m {
        let own = Duration::new(secs, nanos);
        let Some(other) = args
            .first()
            .and_then(super::std_bridge::duration_from_value)
        else {
            bail!("`{}` on Duration takes a Duration argument", name.text);
        };
        let out = match m {
            B::CheckedAdd => own.checked_add(other),
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
        None => bail!("unknown method `{}` on Duration", name.text),
    }
}

/// Width-aware integer methods.
fn int_method(recv: &Value, name: &MethodName, args: &[Value]) -> Option<Result<Value>> {
    let m = name.id;
    // An operand with no i128 image is a u128 past `i128::MAX`; it answers
    // on the native 128-bit cores over raw bits. Checking through the
    // `int_parts` failure keeps this off the hot 64-bit dispatch path.
    let Some((value, mut width)) = recv.int_parts() else {
        return big_int_route(recv, name, args);
    };
    let mut decoded = Vec::with_capacity(args.len());
    for arg in args {
        let Some((arg_value, arg_width)) = arg.int_parts() else {
            return big_int_route(recv, name, args);
        };
        decoded.push(arg_value);
        // Receiver and argument share one type in real Rust, so a width
        // either side states answers for both. A shift amount's own u32 must not redefine the receiver.
        if !super::int_methods::takes_amount_arg(m)
            && let Ok(unified) = super::numeric::unify(width, arg_width)
        {
            width = unified;
        }
    }
    // The in-range 128-bit values decode losslessly, so `value` is already
    // the raw bit pattern the native cores take.
    if width.is_big() {
        let out = super::int_methods::big_int_method(m, width, value, &decoded)?;
        return Some(out.map(|o| int_out(o, width)));
    }
    Some(
        match super::int_methods::int_method(m, width, value, &decoded)? {
            Ok(out) => Ok(int_out(out, width)),
            Err(error) => Err(error),
        },
    )
}

/// The 128-bit method route for a call whose receiver or argument has no
/// i128 image. Answers `None` when no 128-bit operand is present at all,
/// which is any non-integer receiver. Cold: the hot integer dispatch calls
/// it only through the `int_parts` failure path.
#[cold]
fn big_int_route(recv: &Value, name: &MethodName, args: &[Value]) -> Option<Result<Value>> {
    let m = name.id;
    let mut width = match recv {
        Value::Big(_, w) => Some(*w),
        _ => None,
    };
    if width.is_none() {
        width = args.iter().find_map(|v| match v {
            Value::Big(_, w) => Some(*w),
            _ => None,
        });
    }
    let width = width?;
    let bits = big_bits(recv)?;
    let decoded: Option<Vec<i128>> = args.iter().map(big_bits).collect();
    let out = super::int_methods::big_int_method(m, width, bits, &decoded?)?;
    Some(out.map(|o| int_out(o, width)))
}

/// Storage bits of an operand in a 128-bit method call: a `Big` carries
/// them directly, an untagged literal or a tagged value by its value, which
/// is the same bit pattern for everything valid Rust can mix with a
/// 128-bit operand.
fn big_bits(v: &Value) -> Option<i128> {
    match v {
        Value::Big(bits, _) => Some(*bits),
        Value::Int(i) => Some(i128::from(*i)),
        other => other.int_parts().map(|(value, _)| value),
    }
}

/// Materialize an f32 core answer as a runtime value. Called before
/// `bridge_image` widens the receiver, so the result keeps the f32 tag.
fn f32_method(recv: f32, name: BuiltinId, args: &[Value]) -> Result<Option<Value>> {
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

fn scalar_method(recv: &Value, name: &MethodName, args: &[Value]) -> Result<Value> {
    let m = name.id;
    // A conversion that only changes the static type is a no-op on a scalar,
    // and a number never reaches a generic dispatch, so these are answered
    // here. `2.into()` for a `serde_json::Number` is the same 2.
    match m {
        B::ToString => return Ok(Value::str(recv.display())),
        B::Clone | B::Into => return Ok(recv.clone()),
        _ => {}
    }
    // Serde accessors on an already decoded scalar. A json bool arrives as a
    // plain Bool here, so `as_bool` has to answer on it, and an accessor for
    // the wrong type is None rather than an error, matching serde.
    if matches!(
        m,
        B::AsStr
            | B::AsI64
            | B::AsU64
            | B::AsF64
            | B::AsBool
            | B::AsArray
            | B::AsArrayMut
            | B::AsObject
            | B::AsObjectMut
    ) {
        let matched = match (recv, m) {
            (Value::Bool(_), B::AsBool)
            | (Value::Str(_), B::AsStr)
            | (Value::Int(_) | Value::IntW(..), B::AsI64 | B::AsU64)
            | (Value::Float(_), B::AsF64) => true,
            (Value::Int(i), B::AsF64) => {
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
        bail!("unknown method `{}` on a number", name.text);
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
    methods::generic_method(recv, name, args)
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
fn path_closure(path: PathRef, num_params: usize) -> Value {
    Value::Closure(Arc::new(ClosureData {
        chunk: super::bytecode::path_call_chunk(path, num_params),
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
    let read = if name.id.mutates() {
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
    // In-place ascii casing through a `&mut` receiver stores back the same
    // way a grow does. The upper flag reuses the harvested arm literal, a
    // partial literal here would leak a bogus name into the bridge tables.
    if matches!(name.id, B::MakeAsciiUppercase | B::MakeAsciiLowercase) {
        let upper = name.id == B::MakeAsciiUppercase;
        let cased = match &value {
            Value::Str(s) => Some(Value::str(if upper {
                s.to_ascii_uppercase()
            } else {
                s.to_ascii_lowercase()
            })),
            Value::Char(c) => Some(Value::Char(if upper {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            })),
            _ => None,
        };
        if let Some(cased) = cased {
            reference.set(cased);
            return Ok(RefRead::StrGrown);
        }
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
                    Value::Big(v, w) => Some(super::format::SpecNumber::Big {
                        bits: *v,
                        signed: w.is_signed(),
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
                    match vm.user_fmt_text(&value, false)? {
                        Some(text) => text,
                        None => value.display(),
                    }
                };
                let debug_text = if wants_debug {
                    match vm.user_fmt_text(&value, true)? {
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
