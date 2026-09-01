//! The bridge front door, format rendering and method and path dispatch. The per receiver families
//! live in `methods`, `vecmap`, `higher_order` and `iterator`.

use num_traits::AsPrimitive;
use std::f64::consts::{E, PI, TAU};
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use parking_lot::Mutex;

use super::bytecode::{BuiltinId, MethodName, PathId, PathRef};
use super::enum_def::EnumKind;
use super::methods::{self};
use super::native::Native;
use super::shared::Args;
use super::value::{ClosureData, Map, MapKey, Value};
use super::vm::Vm;

impl Vm {
    // path values

    pub(super) fn eval_path_value(&self, path: &PathRef) -> Result<Value> {
        if path.id == PathId::Other {
            return self.user_path_value(path);
        }
        if let Some(v) = path_constant(path.id) {
            return Ok(v);
        }
        // a zero arg constructor like `Vec::new` becomes a nullary closure, anything else a one
        // arg closure
        let arity = usize::from(!matches!(path.id.name(), "new" | "default"));
        Ok(path_closure(path.clone(), arity))
    }

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
            // `struct Marker;` then `Marker`
            if let Some(name) = self
                .unit_structs
                .iter()
                .find(|name| &***name == last || super::resolver::bare(name) == last)
            {
                let type_id = self.impls.type_id(name);
                return Ok(Value::structure(
                    super::value::StructShape::typed(
                        &**name,
                        type_id,
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                    ),
                    Vec::new(),
                ));
            }
            // `.map(strip_html)`, the closure forwards to `dispatch_call`
            if let Some(chunk) = self.user_function(last) {
                return Ok(path_closure(path.clone(), chunk.num_params));
            }
        }
        // A zero arg constructor becomes a nullary closure, a method reference like
        // `Value::as_str` a one arg closure resolved as a UFCS call.
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
        // A `SCREAMING_CASE` tail is a constant. The closure fallback would smuggle a closure
        // into arithmetic.
        if last.chars().any(|c| c.is_ascii_uppercase())
            && !last.chars().any(|c| c.is_ascii_lowercase())
        {
            bail!("unsupported constant `{}`", path.display());
        }
        Ok(path_closure(path.clone(), 1))
    }

    // path calls

    pub(super) fn dispatch_call(
        self: &Arc<Self>,
        path: &PathRef,
        mut args: Vec<Value>,
    ) -> Result<Value> {
        if path.id == PathId::Other {
            return self.dispatch_user_call(path, args);
        }
        // the bridge paths read plain `i64` and `f64`, a width tagged literal arrives widened
        for arg in &mut args {
            if let Some(image) = arg.bridge_image() {
                *arg = image;
            }
        }
        match path.id {
            PathId::Other => return self.dispatch_user_call(path, args),
            PathId::Some => return Ok(Value::some(one(args)?)),
            PathId::Ok => return Ok(Value::ok(one(args)?)),
            PathId::Err => return Ok(Value::err(one(args)?)),
            // the register was cleared at the call site, so this is the last holder and a user
            // `Drop` runs now
            PathId::Drop => {
                self.run_user_drop(one(args)?)?;
                return Ok(Value::Unit);
            }
            // the Ctrl-C handler must reach back into the interpreter to run the script's closure
            PathId::CtrlcSetHandler => {
                let closure = arg(&args, 0)?;
                return Ok(match super::set_ctrlc_handler(closure) {
                    Ok(()) => Value::ok(Value::Unit),
                    Err(e) => Value::err(Value::str(e.to_string())),
                });
            }
            // `sleep` is the 1 thread function that needs no threading
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
            // `tokio::sync::Mutex::lock` is awaited and has no `Result` layer
            PathId::TokioSyncMutexNew => {
                let inner = one(args)?;
                return Ok(super::cell::make_cell(
                    super::value::CellKind::TokioMutex,
                    inner,
                ));
            }
            // `Value::String(s)` is exactly its payload, parsed json is held as native values
            PathId::ValueString
            | PathId::ValueBool
            | PathId::ValueNumber
            | PathId::ValueArray
            | PathId::ValueObject => return one(args),
            _ => {}
        }
        // Native bridges compute in i64 and f64, so they get the plain image. Script level
        // targets keep the real args with width tags.
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

    /// A call the path table doesn't know.
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
        if last == "from"
            && args.len() == 1
            && let Some(chunk) = self.conversion_impl(namespace, &args[0])
        {
            return self.run_chunk(&chunk, &args, &[]);
        }
        // the receiver, if any, is the first argument
        if let Some(chunk) = self.user_method(namespace, last) {
            return self.run_chunk(&chunk, &args, &[]);
        }
        if let Some(v) = self.make_tuple_variant(Some(namespace), last, &args) {
            return Ok(v);
        }
        // UFCS fallback, this is what makes `str::trim` handed to `map` callable.
        // `eval_method` takes the real args so `u8::saturating_add` sees its width.
        if let Some((recv, rest)) = args.split_first() {
            let recv = recv.clone();
            let mut rest = rest.to_vec();
            let name = self.impls.method_name(last);
            return self.eval_method(&recv, &name, &mut rest);
        }
        bail!("unsupported call `{}`", path.display())
    }

    // methods

    /// In stages. The script's own impl method wins over every builtin like an inherent method in
    /// `rustc`, then the any receiver families, the numeric methods at their width, then the per
    /// receiver bridges.
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
        // a shared cell handles its wrapper methods, everything else auto derefs
        if let Value::Cell(kind, slot) = recv {
            if let Some(v) = super::cell::cell_method(*kind, slot, name.id, args)? {
                return Ok(v);
            }
            let inner = slot.lock().clone();
            return self.eval_method(&inner, name, args);
        }
        if let Some(v) = self.user_impl_method(recv, name, args)? {
            return Ok(v);
        }
        if let Some(v) = self.any_receiver_method(recv, name, args)? {
            return Ok(v);
        }
        // before `bridge_image` flattens the receiver to an i64 and forgets its width
        if let Some(result) = int_method(recv, name, args) {
            return result;
        }
        // same for f32, before the image widens it to f64
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
        image_args(recv, name, args)?;
        // A range handles its own few methods first, then the iterator methods through its
        // iterator value.
        let expanded;
        let recv = match recv {
            Value::Range { .. } => {
                if let Some(v) = range_builtin(recv, name, args)? {
                    return Ok(v);
                }
                expanded = self.iterator_value(recv.clone())?;
                &expanded
            }
            _ if self.has_user_next(recv) => {
                expanded = self.iterator_value(recv.clone())?;
                &expanded
            }
            _ => recv,
        };
        if name.id.is_higher_order()
            && let Some(v) = self.higher_order(recv, name.id, &*args)?
        {
            return Ok(v);
        }
        self.method_by_receiver(recv, name, args)
    }

    /// `None` when the type has no such method.
    fn user_impl_method(
        self: &Arc<Self>,
        recv: &Value,
        name: &MethodName,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let Some(chunk) = self
            .impls
            .of_receiver(recv, name.scalar.as_ref())
            .and_then(|methods| methods.get(name))
        else {
            return Ok(None);
        };
        let chunk = chunk.clone();
        let mut full = Vec::with_capacity(args.len() + 1);
        full.push(recv.clone());
        full.extend(args.iter().cloned());
        self.run_chunk(&chunk, &full, &[]).map(Some)
    }

    /// The any receiver methods. They run before `bridge_image`, a u64 past `i64::MAX` saturates
    /// there and would look like an i64.
    fn any_receiver_method(
        self: &Arc<Self>,
        recv: &Value,
        name: &MethodName,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let tagged = matches!(recv, Value::IntW(..) | Value::F32(_));
        Ok(match name.id {
            BuiltinId::ToString => match self.user_fmt_text(recv, false)? {
                Some(text) => Some(Value::str(text)),
                None if tagged => Some(Value::str(recv.display())),
                None => None,
            },
            BuiltinId::Clone if tagged => Some(recv.clone()),
            _ => methods::json_type_test(recv, name)
                .or_else(|| methods::json_value_method(recv, name, args)),
        })
    }

    fn method_by_receiver(
        self: &Arc<Self>,
        recv: &Value,
        name: &MethodName,
        args: &mut [Value],
    ) -> Result<Value> {
        match recv {
            Value::Str(s) => methods::str_method(s, name, args),
            Value::Vec(v) => {
                // a lazy `extend` argument is drained here, the vec method itself can't read one
                if matches!(name.id, BuiltinId::Extend | BuiltinId::ExtendFromSlice)
                    && let Some(first) = args.first()
                    && !matches!(first, Value::Vec(_))
                {
                    let items = self.drain_items(first.clone())?;
                    args[0] = Value::vec(items);
                }
                super::vecmap::vec_method(v, name, args)
            }
            Value::Map(map, kind) => super::vecmap::map_method(map, *kind, name, args),
            Value::Enum { def, .. } if def.kind == EnumKind::Option => {
                methods::opt_method(recv, name, args)
            }
            Value::Enum { def, .. } if def.kind == EnumKind::Result => {
                methods::res_method(recv, name, args)
            }
            Value::Enum { .. } => methods::generic_method(recv, name, args),
            Value::Struct(st) => {
                if let Some(res) = super::http::http_method(recv, name, args) {
                    return res;
                }
                if super::ratatui::is_ratatui_struct(st.name()) {
                    return super::ratatui::struct_method(st, name, args);
                }
                Self::bridge_struct_method(recv, st, name, args)
            }
            Value::Native(native) => {
                // one lock to pick the family, the families lock again on their own
                let family = match &*native.lock() {
                    Native::Iterator(_) => NativeFamily::Iterator,
                    Native::HttpClient(_) | Native::BlockingHttpClient(_) => NativeFamily::Http,
                    _ => NativeFamily::Other,
                };
                match family {
                    NativeFamily::Iterator => {
                        if let Some(v) = self.iterator_method(native, name, args)? {
                            return Ok(v);
                        }
                    }
                    NativeFamily::Http => {
                        if let Some(res) = super::http::http_method(recv, name, args) {
                            return res;
                        }
                    }
                    NativeFamily::Other | NativeFamily::Entry(..) | NativeFamily::Regex => {}
                }
                Self::native_method(native, name, args)
            }
            Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Char(_) => {
                scalar_method(recv, name, args)
            }
            other => methods::generic_method(other, name, args),
        }
    }

    fn bridge_struct_method(
        recv: &Value,
        st: &Arc<super::value::StructData>,
        name: &MethodName,
        args: &mut [Value],
    ) -> Result<Value> {
        match &**st.name() {
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
            "Permissions" => match name.id {
                BuiltinId::Mode => Ok(st.get("mode").unwrap_or(Value::Int(0))),
                BuiltinId::Readonly => Ok(st.get("readonly").unwrap_or(Value::Bool(false))),
                BuiltinId::SetReadonly => Ok(Value::Unit),
                _ => bail!("unknown method `{name}` on Permissions"),
            },
            "Rng" => super::crates_bridge::rng_method(name, args),
            "Base64Engine" => super::crates_bridge::base64_method(st, name, args),
            "Element" => super::xmltree_bridge::element_method(st, name, args),
            "RegKey" => super::winreg_bridge::winreg_method(st, name, args),
            "ServiceManager" => super::service_bridge::manager_method(st, name, args),
            "Service" => super::service_bridge::service_method(st, name, args),
            "WmiConnection" => super::wmi_bridge::wmi_method(st, name, args),
            _ => methods::generic_method(recv, name, args),
        }
    }

    fn native_method(
        native: &Arc<Mutex<Native>>,
        name: &MethodName,
        args: &mut [Value],
    ) -> Result<Value> {
        // The io family walks a chain of handle locks. A regex match in a loop would pay all of
        // them for `start`, so the value like handles route on one lock here.
        let family = match &*native.lock() {
            Native::Entry { map, key } => NativeFamily::Entry(map.clone(), key.clone()),
            Native::Instant(instant) if name.id == BuiltinId::Elapsed => {
                return Ok(super::std_bridge::make_duration(instant.elapsed()));
            }
            Native::Regex(_) | Native::RegexMatch(_) | Native::RegexCaptures(_) => {
                NativeFamily::Regex
            }
            _ => NativeFamily::Other,
        };
        match family {
            NativeFamily::Entry(map, key) => {
                return methods::entry_method(&map, &key, name, args);
            }
            NativeFamily::Regex => {
                if let Some(v) = super::regex_bridge::regex_native_method(native, name, args)? {
                    return Ok(v);
                }
            }
            NativeFamily::Other => {
                if let Some(v) = super::native_methods::native_method(native, name, args)? {
                    return Ok(v);
                }
            }
            NativeFamily::Iterator | NativeFamily::Http => {
                unreachable!("picked in method_by_receiver")
            }
        }
        methods::generic_method(&Value::Native(native.clone()), name, args)
    }
}

/// Which handler a `Native` receiver routes to, decided under one lock.
enum NativeFamily {
    Iterator,
    Http,
    Entry(Map, MapKey),
    Regex,
    Other,
}

// free helpers

/// None for a path that names a function, that becomes a closure.
fn path_constant(id: PathId) -> Option<Value> {
    let text = match id {
        PathId::UnixEpoch => return Some(Native::SystemTime(std::time::UNIX_EPOCH).wrap()),
        // a json null is None, same mapping as the parser
        PathId::ValueNull => return Some(Value::none()),
        PathId::ConstsPi => return Some(Value::Float(PI)),
        PathId::ConstsTau => return Some(Value::Float(TAU)),
        PathId::ConstsE => return Some(Value::Float(E)),
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

/// Flatten width tagged arguments to the i64 and f64 images the bridges compute in. Methods that hand
/// arguments through or store them, like `unwrap_or`, `then_some`, `fold` and the containers,
/// keep the tags.
fn image_args(recv: &Value, name: &MethodName, args: &mut [Value]) -> Result<()> {
    // `bridge_image` saturates a count past `i64::MAX` to `isize::MAX`, and `"0".repeat(isize::MAX)`
    // is a failed allocation, not the `capacity overflow` panic the real count gives
    if name.id == BuiltinId::Repeat
        && let Some(count) = args.first()
        && count
            .int_parts()
            .is_some_and(|(n, _)| n > i128::from(i64::MAX))
    {
        let empty = match recv {
            Value::Str(s) => s.is_empty(),
            Value::Vec(v) => v.lock().is_empty(),
            _ => false,
        };
        if !empty {
            bail!("capacity overflow");
        }
    }
    let hands_args_through = matches!(
        recv,
        Value::Enum { .. } | Value::Bool(_) | Value::Vec(_) | Value::Map(..)
    ) || matches!(recv, Value::Native(n) if matches!(&*n.lock(), Native::Entry { .. }))
        || name.id == BuiltinId::Fold;
    if hands_args_through {
        return Ok(());
    }
    for arg in args.iter_mut() {
        if let Some(image) = arg.bridge_image() {
            *arg = image;
        }
    }
    Ok(())
}

fn one(args: Vec<Value>) -> Result<Value> {
    args.into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("expected one argument"))
}

/// A missing argument is an interpreter bug, the checker rules it out.
pub(super) fn arg(args: &[Value], i: usize) -> Result<Value> {
    args.get(i)
        .cloned()
        .ok_or_else(|| anyhow!("missing argument {}", i + 1))
}

/// So a path written as a value can be handed to `map` or `and_then`.
fn path_closure(path: PathRef, num_params: usize) -> Value {
    Value::Closure(Arc::new(ClosureData {
        chunk: super::bytecode::path_call_chunk(path, num_params),
        captured: Vec::new(),
    }))
}

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

enum RefRead {
    Value(Value),
    /// a string grow already ran and stored back, nothing left to dispatch
    StrGrown,
}

/// A mutating method splits the referenced slot first. A string grows its own buffer, so the
/// grown buffer stores back through the reference.
fn deref_receiver(
    reference: &super::value::ValueRef,
    name: &MethodName,
    args: &[Value],
) -> Result<RefRead> {
    let Some(value) = reference.get() else {
        bail!("method call through a dangling reference");
    };
    if let Value::Str(s) = &value
        && matches!(name.id, BuiltinId::Push | BuiltinId::PushStr)
    {
        let mut grown = s.clone();
        methods::str_grow(&mut grown, name.id, &arg(args, 0)?)?;
        reference.set(Value::Str(grown));
        return Ok(RefRead::StrGrown);
    }
    // a reference is a place, so `clear` is `String::clear` and never the colored one
    if matches!(value, Value::Str(_)) && name.id == BuiltinId::Clear && args.is_empty() {
        reference.set(Value::str(String::new()));
        return Ok(RefRead::StrGrown);
    }
    // The upper flag reuses the harvested arm literal. A partial literal would leak a bogus name
    // into the bridge tables.
    if matches!(
        name.id,
        BuiltinId::MakeAsciiUppercase | BuiltinId::MakeAsciiLowercase
    ) {
        let upper = name.id == BuiltinId::MakeAsciiUppercase;
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

mod path_calls;
mod scalar_dispatch;
pub(in crate::interpreter) use scalar_dispatch::int_method;
mod template;
mod user_impls;

use path_calls::{
    bridge_call, datetime_method, duration_method, exitstatus_method, numeric_limit, output_method,
    range_builtin,
};
use scalar_dispatch::{f32_method, scalar_method};
