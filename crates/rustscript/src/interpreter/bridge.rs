//! The bridge front door, format rendering and method and path dispatch.
//! The per receiver families live in `methods`, `vecmap`, `higher_order`
//! and `iterator`.

use num_traits::AsPrimitive;
use std::f64::consts::PI;
use std::mem::take;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use parking_lot::Mutex;

use super::bytecode::Chunk;
use super::bytecode::{BuiltinId, MethodName, PathId, PathRef};
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

    /// None when the type has no user `Display` or `Debug` impl.
    pub(super) fn user_fmt_text(
        self: &Arc<Self>,
        v: &Value,
        debug: bool,
    ) -> Result<Option<String>> {
        Ok(self.user_fmt(v, debug)?.map(|(text, _)| text))
    }

    /// `user_fmt_text` plus whether the impl padded through `f.pad`.
    pub(super) fn user_fmt(
        self: &Arc<Self>,
        v: &Value,
        debug: bool,
    ) -> Result<Option<(String, bool)>> {
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
        let handle = Arc::new(parking_lot::Mutex::new(Native::Fmt {
            text: String::new(),
            padded: false,
        }));
        let args = vec![v.clone(), Value::Native(handle.clone())];
        self.run_chunk(&chunk, &args, &[])?;
        let out = match &*handle.lock() {
            Native::Fmt { text, padded } => (text.clone(), *padded),
            _ => (String::new(), false),
        };
        Ok(Some(out))
    }

    /// Runs `Drop::drop` only when this is the last holder, another holder
    /// means moved or shared. Containers hand their contents on. A shared
    /// cycle never reaches one holder, so it leaks like a real `Rc` cycle.
    pub(super) fn run_user_drop(self: &Arc<Self>, value: Value) -> Result<()> {
        match value {
            Value::Struct(s) => {
                if Arc::strong_count(&s) != 1 {
                    return Ok(());
                }
                self.run_drop_impl(Value::Struct(s.clone()))?;
                // The impl could have stored a clone of self somewhere.
                if Arc::strong_count(&s) != 1 {
                    return Ok(());
                }
                // Fields drop after `Drop::drop` in declaration order.
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
        // A zero arg constructor like `Vec::new` becomes a nullary closure,
        // anything else a one arg closure.
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
            // `struct Marker;` then `Marker`.
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
            // `.map(strip_html)`, the closure forwards to `dispatch_call`.
            if let Some(chunk) = self.user_function(last) {
                return Ok(path_closure(path.clone(), chunk.num_params));
            }
        }
        // A zero arg constructor becomes a nullary closure, a method reference
        // like `Value::as_str` a one arg closure resolved as a UFCS call.
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
        // A `SCREAMING_CASE` tail is a constant, the closure fallback would
        // smuggle a closure into arithmetic.
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
            // The register was cleared at the call site, so this is the last
            // holder and a user `Drop` runs now.
            PathId::Drop => {
                self.run_user_drop(one(args)?)?;
                return Ok(Value::Unit);
            }
            // The Ctrl-C handler must reach back into the interpreter to run
            // the script's closure.
            PathId::CtrlcSetHandler => {
                let closure = arg(&args, 0)?;
                return Ok(match super::set_ctrlc_handler(closure) {
                    Ok(()) => Value::ok(Value::Unit),
                    Err(e) => Value::err(Value::str(e.to_string())),
                });
            }
            // `sleep` is the one thread function that needs no threading.
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
            // `tokio::sync::Mutex::lock` is awaited and has no `Result` layer.
            PathId::TokioSyncMutexNew => {
                let inner = one(args)?;
                return Ok(super::cell::make_cell(
                    super::value::CellKind::TokioMutex,
                    inner,
                ));
            }
            // `Value::String(s)` is exactly its payload, parsed json is held as
            // native values.
            PathId::ValueString
            | PathId::ValueBool
            | PathId::ValueNumber
            | PathId::ValueArray
            | PathId::ValueObject => return one(args),
            _ => {}
        }
        // Native bridges compute in i64 and f64, so they get the plain image.
        // Script level targets keep the real args with width tags.
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

    /// A call the path table does not name.
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
        // The receiver, if any, is the first argument.
        if let Some(chunk) = self.user_method(namespace, last) {
            return self.run_chunk(&chunk, &args, &[]);
        }
        if let Some(v) = self.make_tuple_variant(Some(namespace), last, &args) {
            return Ok(v);
        }
        // UFCS fallback, what makes `str::trim` handed to `map` callable.
        // `eval_method` takes the real args so `u8::saturating_add` sees its
        // width.
        if let Some((recv, rest)) = args.split_first() {
            let recv = recv.clone();
            let mut rest = rest.to_vec();
            let name = self.impls.method_name(last);
            return self.eval_method(&recv, &name, &mut rest);
        }
        bail!("unsupported call `{}`", path.display())
    }

    // -- methods -----------------------------------------------------------

    /// In stages. The script's own impl method wins over every builtin like
    /// an inherent method in `rustc`, then the any receiver families, the
    /// numeric methods at their width, and the per receiver bridges.
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
        // A shared cell answers its wrapper methods, everything else auto
        // derefs.
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
        // Before `bridge_image` flattens the receiver to an i64 and forgets
        // its width.
        if let Some(result) = int_method(recv, name, args) {
            return result;
        }
        // f32 likewise, before the image widens it to f64.
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
        image_args(recv, name, args);
        // A range answers its own handful of methods first, then the iterator
        // methods through its iterator value.
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

    /// `None` when the type declares no such method.
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

    /// The any receiver methods. They run before `bridge_image`, since a u64
    /// past `i64::MAX` saturates there and would claim to be an i64.
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
                // A lazy `extend` argument is drained here, the vec method
                // itself cannot read one.
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
                if matches!(&*native.lock(), Native::Iterator(_))
                    && let Some(v) = self.iterator_method(native, name, args)?
                {
                    return Ok(v);
                }
                if let Some(res) = super::http::http_method(recv, name, args) {
                    return res;
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
        let entry = match &*native.lock() {
            Native::Entry { map, key } => Some((map.clone(), key.clone())),
            _ => None,
        };
        if let Some((map, key)) = entry {
            return methods::entry_method(&map, &key, name, args);
        }
        if let Native::Instant(instant) = &*native.lock()
            && name.id == BuiltinId::Elapsed
        {
            return Ok(super::std_bridge::make_duration(instant.elapsed()));
        }
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

/// None for a path that names a function, which becomes a closure.
fn path_constant(id: PathId) -> Option<Value> {
    let text = match id {
        PathId::UnixEpoch => return Some(Native::SystemTime(std::time::UNIX_EPOCH).wrap()),
        // A json null is None, the same mapping the parser uses.
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

/// Flatten width tagged arguments to the i64 and f64 images the bridges
/// compute in. Methods that hand arguments through or store them, like
/// `unwrap_or`, `then_some`, `fold` and the containers, keep the tags.
fn image_args(recv: &Value, name: &MethodName, args: &mut [Value]) {
    let hands_args_through = matches!(
        recv,
        Value::Enum { .. } | Value::Bool(_) | Value::Vec(_) | Value::Map(..)
    ) || matches!(recv, Value::Native(n) if matches!(&*n.lock(), Native::Entry { .. }))
        || name.id == BuiltinId::Fold;
    if hands_args_through {
        return;
    }
    for arg in args.iter_mut() {
        if let Some(image) = arg.bridge_image() {
            *arg = image;
        }
    }
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

/// Answered before the range expands to its iterator value.
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
fn numeric_limit(id: PathId) -> Option<Value> {
    use super::numeric::IntWidth;
    Some(match id {
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
        // u128 bounds are reinterpreted bits in `Value::Big`.
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

/// None when no bridge answers the id.
fn bridge_call(id: PathId, args: &[Value]) -> Result<Option<Value>> {
    match id {
        PathId::UtcNow | PathId::LocalNow => {
            return Ok(Some(now_datetime(id == PathId::LocalNow)));
        }
        PathId::DateTimeParseFromRfc3339 => {
            return Ok(Some(match parse_rfc3339(&arg(args, 0)?.display()) {
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
        BuiltinId::Status | BuiltinId::Stdout | BuiltinId::Stderr => s
            .get(m.name())
            .ok_or_else(|| anyhow!("Output has no `{m}` field"))?,
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
    if let BuiltinId::CheckedAdd | BuiltinId::CheckedSub = m {
        let own = Duration::new(secs, nanos);
        let Some(other) = args
            .first()
            .and_then(super::std_bridge::duration_from_value)
        else {
            bail!("`{}` on Duration takes a Duration argument", name.text);
        };
        let out = match m {
            BuiltinId::CheckedAdd => own.checked_add(other),
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

fn int_method(recv: &Value, name: &MethodName, args: &[Value]) -> Option<Result<Value>> {
    let m = name.id;
    // An operand with no i128 image is a u128 past `i128::MAX`. Checking
    // through the `int_parts` failure keeps this off the hot path.
    let Some((value, mut width)) = recv.int_parts() else {
        return big_int_route(recv, name, args);
    };
    let mut decoded = Vec::with_capacity(args.len());
    for arg in args {
        let Some((arg_value, arg_width)) = arg.int_parts() else {
            return big_int_route(recv, name, args);
        };
        decoded.push(arg_value);
        // Receiver and argument share one type, so either width answers for
        // both. A shift amount's u32 must not redefine the receiver.
        if !super::int_methods::takes_amount_arg(m)
            && let Ok(unified) = super::numeric::unify(width, arg_width)
        {
            width = unified;
        }
    }
    // `value` is already the raw bit pattern the native cores take.
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

/// `None` when no 128 bit operand is present. Cold, reached only through
/// the `int_parts` failure path.
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

/// A `Big` carries the bits directly, anything else by its value, which is
/// the same pattern for everything valid Rust can mix with it.
fn big_bits(v: &Value) -> Option<i128> {
    match v {
        Value::Big(bits, _) => Some(*bits),
        Value::Int(i) => Some(i128::from(*i)),
        other => other.int_parts().map(|(value, _)| value),
    }
}

/// Called before `bridge_image` widens the receiver, so the result keeps
/// the f32 tag.
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
        // Counts are u32, or `!x.count_ones()` prints -1 instead of
        // 4294967295.
        IntOut::Count(count) => {
            Value::int_of_width(i128::from(count), super::numeric::IntWidth::U32)
        }
        IntOut::Bool(value) => Value::Bool(value),
        IntOut::Checked(Some(value)) => Value::some(Value::int_of_width(value, width)),
        IntOut::Checked(None) | IntOut::CheckedCount(None) => Value::none(),
        IntOut::SomeFloat(value) => Value::some(Value::Float(value)),
        IntOut::Ordering(ordering) => make_ordering(ordering),
        IntOut::Bytes(bytes) => Value::vec(
            bytes
                .into_iter()
                .map(|byte| Value::Int(i64::from(byte)))
                .collect(),
        ),
        IntOut::Overflowing(value, wrapped) => Value::tuple(vec![
            Value::int_of_width(value, width),
            Value::Bool(wrapped),
        ]),
        IntOut::CheckedCount(Some(count)) => Value::some(Value::int_of_width(
            i128::from(count),
            super::numeric::IntWidth::U32,
        )),
    }
}

fn scalar_method(recv: &Value, name: &MethodName, args: &[Value]) -> Result<Value> {
    let m = name.id;
    // A conversion that only changes the static type is a no-op on a scalar.
    match m {
        BuiltinId::ToString => return Ok(Value::str(recv.display())),
        BuiltinId::Clone | BuiltinId::Into => return Ok(recv.clone()),
        _ => {}
    }
    // Serde accessors on a decoded scalar. A wrong type accessor is None,
    // matching serde.
    if matches!(
        m,
        BuiltinId::AsStr
            | BuiltinId::AsI64
            | BuiltinId::AsU64
            | BuiltinId::AsF64
            | BuiltinId::AsBool
            | BuiltinId::AsArray
            | BuiltinId::AsArrayMut
            | BuiltinId::AsObject
            | BuiltinId::AsObjectMut
    ) {
        let matched = match (recv, m) {
            (Value::Bool(_), BuiltinId::AsBool)
            | (Value::Str(_), BuiltinId::AsStr)
            | (Value::Int(_) | Value::IntW(..), BuiltinId::AsI64 | BuiltinId::AsU64)
            | (Value::Float(_), BuiltinId::AsF64) => true,
            (Value::Int(i), BuiltinId::AsF64) => {
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
            CharOut::USize(n) => super::shared::usize_value(n),
        });
    }
    methods::generic_method(recv, name, args)
}

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
    /// A string grow already ran and stored back, nothing left to dispatch.
    StrGrown,
}

/// A mutating method splits the referenced slot first. A string grows its
/// own buffer, so the grown buffer stores back through the reference.
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
        methods::str_grow(&mut grown, name.id, &arg(args, 0)?)?;
        reference.set(Value::Str(grown));
        return Ok(RefRead::StrGrown);
    }
    // A reference is a place, so `clear` is `String::clear`, never the
    // colored crate's.
    if matches!(value, Value::Str(_)) && name.id == BuiltinId::Clear && args.is_empty() {
        reference.set(Value::str(String::new()));
        return Ok(RefRead::StrGrown);
    }
    // The upper flag reuses the harvested arm literal, a partial literal
    // would leak a bogus name into the bridge tables.
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
                out.push_str(&render_placeholder(
                    vm,
                    &spec,
                    &mut next_pos,
                    positional,
                    named,
                )?);
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

fn render_placeholder(
    vm: &Arc<Vm>,
    spec: &str,
    next_pos: &mut usize,
    positional: &[Value],
    named: &[(&str, Value)],
) -> Result<String> {
    let (name, fmt) = spec.split_once(':').unwrap_or((spec, ""));
    // `{:.*}` takes its precision from the next positional argument.
    let fmt = if fmt.contains(".*") {
        let precision = match resolve_arg("", next_pos, positional, named)? {
            Value::Int(i) => i,
            ref other @ Value::IntW(..) => other
                .untag_int()
                .ok_or_else(|| anyhow::anyhow!("format precision out of range"))?,
            other => {
                bail!(
                    "format precision must be an integer, got {}",
                    other.type_name()
                )
            }
        };
        fmt.replace(".*", &format!(".{precision}"))
    } else {
        fmt.to_string()
    };
    let fmt = fmt.as_str();
    let value = resolve_arg(name, next_pos, positional, named)?;
    // A `{:w$}` width names another argument.
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
    // Only the form the spec asks for runs, an impl may have side effects.
    let wants_debug = fmt.contains('?');
    // `write!` ignores the caller's width, `f.pad` honors it.
    let mut user_padded = None;
    let display_text = if wants_debug {
        String::new()
    } else {
        match vm.user_fmt(&value, false)? {
            Some((text, padded)) => {
                user_padded = Some(padded);
                text
            }
            None => value.display(),
        }
    };
    let mut user_debug = false;
    let debug_text = if !wants_debug {
        String::new()
    } else if let Some((text, padded)) = vm.user_fmt(&value, true)? {
        user_debug = true;
        user_padded = Some(padded);
        text
    } else {
        // The flags reach every leaf.
        let leaf: String = fmt.chars().filter(|c| !matches!(c, '#' | '?')).collect();
        super::debug_fmt::render(
            &value,
            &super::debug_fmt::DebugOpts {
                pretty: fmt.contains('#'),
                leaf: &leaf,
            },
        )
    };
    // The debug renderer applied the spec at every leaf already.
    if wants_debug && !user_debug {
        return Ok(debug_text);
    }
    if user_padded == Some(false) {
        return Ok(if wants_debug {
            debug_text
        } else {
            display_text
        });
    }
    Ok(super::format::apply_spec(
        &fmt,
        &display_text,
        &debug_text,
        number,
        user_padded.is_some(),
    ))
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
