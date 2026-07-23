use std::f64::consts::PI;
use std::rc::Rc;

use anyhow::{Result, bail};

use super::bytecode::{BuiltinId, Chunk, MethodName, Op};

use super::Interp;
use super::value::{ClosureData, StructShape, Value};

use super::crates_bridge::*;
use super::http::*;
use super::jwt_bridge::*;
use super::methods::*;
use super::process::*;
use super::regex_bridge::*;
use super::service_bridge::{manager_method, service_const, service_method, service_variant};
use super::std_bridge::*;
use super::winreg_bridge::{winreg_const, winreg_method};
use super::wmi_bridge::wmi_method;

impl Interp {
    /// Resolve a path used as a value: `None`, a unit enum variant, a constant,
    /// or a bare constructor wrapped as a closure.
    pub(super) fn eval_path_value(&self, segs: &[String]) -> Result<Value> {
        if segs.len() == 1 {
            let name = &segs[0];
            if name == "None" {
                return Ok(Value::none());
            }
            if let Some(v) = base64_engine(name) {
                return Ok(v);
            }
            if let Some(v) = winreg_const(name) {
                return Ok(v);
            }
            if let Some(v) = service_const(name) {
                return Ok(v);
            }
            if let Some(v) = self.unit_variant(None, name) {
                return Ok(v);
            }
            // A unit struct used as a value, `struct Marker;` then `Marker`.
            if let Some(def) = self.structs().get(name.as_str())
                && matches!(def.ast.fields, syn::Fields::Unit)
            {
                return Ok(Value::structure(
                    StructShape::new(name.as_str(), Vec::new()),
                    Vec::new(),
                ));
            }
            // A `use`d constant like `use std::env::consts::OS` then bare `OS`
            // resolves through its full path.
            if let Some(full) = self.uses.get(name.as_str())
                && full.len() > 1
            {
                return self.eval_path_value(full);
            }
            bail!("unknown variable `{name}`");
        }

        let last = &segs[segs.len() - 1];
        let ty = &segs[segs.len() - 2];
        if ty == "Option" && last == "None" {
            return Ok(Value::none());
        }
        // Path-value constants from std and bridged crates.
        if ty == "consts" {
            match last.as_str() {
                "PI" => return Ok(Value::Float(PI)),
                "OS" => return Ok(Value::str(std::env::consts::OS)),
                "ARCH" => return Ok(Value::str(std::env::consts::ARCH)),
                "FAMILY" => return Ok(Value::str(std::env::consts::FAMILY)),
                "EXE_EXTENSION" => return Ok(Value::str(std::env::consts::EXE_EXTENSION)),
                "EXE_SUFFIX" => return Ok(Value::str(std::env::consts::EXE_SUFFIX)),
                _ => {}
            }
        }
        if let Some(v) = int_limit(ty, last) {
            return Ok(v);
        }
        if let Some(v) = base64_engine(last) {
            return Ok(v);
        }
        if let Some(v) = winreg_const(last) {
            return Ok(v);
        }
        if let Some(v) = service_variant(ty, last) {
            return Ok(v);
        }
        if let Some(v) = service_const(last) {
            return Ok(v);
        }
        if ty == "Ordering" {
            use std::cmp::Ordering::*;
            return Ok(make_ordering(match last.as_str() {
                "Less" => Less,
                "Greater" => Greater,
                _ => Equal,
            }));
        }
        if let Some(v) = self.unit_variant(Some(ty), last) {
            return Ok(v);
        }
        if let Some(v) = jwt_algorithm(ty, last) {
            return Ok(v);
        }
        // A json null is None here, the same mapping the parser uses, so
        // `serde_json::Value::Null` written in a script lands on the same
        // value. Without this it falls through to the closure fallback below
        // and every later accessor on it reports a method on a closure.
        if ty == "Value" && last == "Null" {
            return Ok(Value::none());
        }
        // A path used as a function value. A zero-arg constructor like
        // `Vec::new` handed to `or_insert_with` becomes a nullary closure.
        // Anything else, a method reference like `str::trim` or a one-arg
        // constructor like `PathBuf::from`, becomes a one-arg closure that
        // forwards its argument to the path call.
        if matches!(last.as_str(), "new" | "default") {
            return Ok(path_call_closure(segs.to_vec(), 0));
        }
        let function = segs.join("::");
        if let Some(chunk) = self
            .user_method(ty, last)
            .or_else(|| self.user_function(&function))
            .or_else(|| self.user_function(last))
        {
            return Ok(path_call_closure(segs.to_vec(), chunk.num_params));
        }
        Ok(path_call_closure(segs.to_vec(), 1))
    }

    fn unit_variant(&self, enum_name: Option<&str>, variant: &str) -> Option<Value> {
        for (name, def) in self.enums() {
            // The wanted enum may be a canonical key or a bare source name.
            if let Some(want) = enum_name
                && want != &**name
                && want != super::resolver::bare(name)
            {
                continue;
            }
            if def
                .variants
                .iter()
                .any(|v| v.ident == variant && matches!(v.fields, syn::Fields::Unit))
            {
                return Some(Value::Enum {
                    enum_name: name.clone(),
                    variant: variant.into(),
                    data: Value::empty_data(),
                });
            }
        }
        None
    }

    pub(super) fn dispatch_call(&self, segs: &[String], args: Vec<Value>) -> Result<Value> {
        let canon = self.canonical(segs);

        if canon.len() == 1 {
            let name = &canon[0];
            match name.as_str() {
                "Some" => return Ok(Value::some(one(args)?)),
                "Ok" => return Ok(Value::ok(one(args)?)),
                "Err" => return Ok(Value::err(one(args)?)),
                // Values die with their register and file writes are
                // unbuffered, so discarding is enough.
                "drop" => return Ok(Value::Unit),
                _ => {}
            }
            if let Some(chunk) = self.user_function(name) {
                return self.run_chunk(&chunk, &args, &[]);
            }
            if self.structs().contains_key(name.as_str()) {
                return self.make_tuple_struct(name, args);
            }
            if let Some(v) = self.make_tuple_variant(None, name, &args) {
                return v;
            }
            bail!("unknown function `{name}`");
        }

        // A namespaced call, `module::func` or `Type::func`. Match on the last
        // two segments so `use` shortenings and full paths behave the same.
        let last = &canon[canon.len() - 1];
        let ns = &canon[canon.len() - 2];
        // The Ctrl-C handler must reach back into the interpreter to run the
        // script's own closure, so it cannot go through the plain native call.
        if ns == "ctrlc" && last == "set_handler" {
            let closure = args.first().cloned().unwrap_or(Value::Unit);
            return Ok(match super::set_ctrlc_handler(closure) {
                Ok(()) => Value::ok(Value::Unit),
                Err(e) => Value::err(Value::str(e.to_string())),
            });
        }
        if ns == "thread" {
            // sleep is the one thread function that needs no threading. Polling
            // an asynchronous thing, a service reaching Running for example,
            // needs it, and rejecting it would push scripts to spin instead.
            if last == "sleep" {
                let Some(d) = args.first().and_then(duration_from_value) else {
                    bail!("thread::sleep takes a Duration");
                };
                std::thread::sleep(d);
                return Ok(Value::Unit);
            }
            bail!(
                "std::thread is not supported beyond sleep, use #[tokio::main] with tokio::spawn"
            );
        }
        // reqwest paths need the whole canonical path, since a blocking call
        // has three or four segments, so route them before the two-segment
        // native dispatch.
        if canon.first().map(String::as_str) == Some("reqwest") {
            return super::http::reqwest_call(&canon, &args);
        }
        if let Some(chunk) = self.user_function(&canon.join("::")) {
            return self.run_chunk(&chunk, &args, &[]);
        }
        if let Some(v) = native_call(ns, last, &args)? {
            return Ok(v);
        }
        // A method on a user type, `Type::assoc(..)` or UFCS `Type::method(recv, ..)`.
        // The receiver, if any, is simply the first argument, matching param 0.
        if let Some(chunk) = self.user_method(ns, last) {
            return self.run_chunk(&chunk, &args, &[]);
        }
        if let Some(v) = assoc_fn(ns, last, &args)? {
            return Ok(v);
        }
        if let Some(v) = self.make_tuple_variant(Some(ns), last, &args) {
            return v;
        }
        // UFCS fallback: `Type::method(recv, ..)` dispatches `method` on the
        // receiver. This is what makes a method reference used as a value, like
        // `str::trim` handed to `map`, callable, since the path call forwards
        // its argument here.
        if let Some((recv, rest)) = args.split_first() {
            let recv = recv.clone();
            let mut rest = rest.to_vec();
            let name = MethodName {
                id: BuiltinId::resolve(last),
                text: last.clone(),
            };
            return self.eval_method(&recv, &name, &mut rest);
        }
        bail!("unsupported call `{}`", canon.join("::"));
    }

    /// Expand the first path segment through the `use` table.
    pub(super) fn canonical(&self, segs: &[String]) -> Vec<String> {
        if let Some(full) = self.uses.get(&segs[0]) {
            let mut out = full.clone();
            out.extend_from_slice(&segs[1..]);
            out
        } else {
            segs.to_vec()
        }
    }

    fn make_tuple_struct(&self, name: &str, args: Vec<Value>) -> Result<Value> {
        let fields = (0..args.len()).map(|i| i.to_string().into()).collect();
        Ok(Value::structure(StructShape::new(name, fields), args))
    }

    fn make_tuple_variant(
        &self,
        enum_name: Option<&str>,
        variant: &str,
        args: &[Value],
    ) -> Option<Result<Value>> {
        for (name, def) in self.enums() {
            if let Some(want) = enum_name
                && want != &**name
                && want != super::resolver::bare(name)
            {
                continue;
            }
            if def.variants.iter().any(|v| v.ident == variant) {
                return Some(Ok(Value::Enum {
                    enum_name: name.clone(),
                    variant: variant.into(),
                    data: args.iter().cloned().collect(),
                }));
            }
        }
        None
    }

    pub(super) fn eval_method(
        &self,
        recv: &Value,
        name: &MethodName,
        args: &mut [Value],
    ) -> Result<Value> {
        if let Value::Range {
            start,
            end,
            inclusive,
        } = recv
        {
            match name.id {
                BuiltinId::Clone => return Ok(recv.clone()),
                BuiltinId::Contains => {
                    let Some(Value::Int(value)) = args.first() else {
                        bail!("range contains needs an integer");
                    };
                    return Ok(Value::Bool(if *inclusive {
                        *value >= *start && *value <= *end
                    } else {
                        *value >= *start && *value < *end
                    }));
                }
                BuiltinId::Len | BuiltinId::Count => {
                    let extra = i64::from(*inclusive && end >= start);
                    return Ok(Value::Int(end.saturating_sub(*start) + extra));
                }
                BuiltinId::IsEmpty => {
                    return Ok(Value::Bool(if *inclusive {
                        start > end
                    } else {
                        start >= end
                    }));
                }
                _ => {}
            }
        }
        // A method on a range acts on its iterator value.
        let expanded;
        let recv = if matches!(recv, Value::Range { .. }) {
            expanded = self.iterator_value(recv.clone())?;
            &expanded
        } else {
            recv
        };
        if let Value::Native(iterator) = recv
            && matches!(&*iterator.borrow(), super::native::Native::Iterator(_))
            && let Some(value) = self.iterator_method(iterator, name, args)?
        {
            return Ok(value);
        }
        // User methods only exist on structs and enums, and only when the
        // script defined any at all, so skip the keyed lookup otherwise.
        if !self.methods.is_empty() {
            let type_name = match recv {
                Value::Struct(s) => Some(s.name().clone()),
                Value::Enum { enum_name, .. } => Some(enum_name.clone()),
                _ => None,
            };
            if let Some(tn) = &type_name
                && let Some(chunk) = self.user_method(tn, &name.text)
            {
                // The receiver is param 0, followed by the call arguments.
                let mut full = Vec::with_capacity(args.len() + 1);
                full.push(recv.clone());
                full.extend_from_slice(args);
                return self.run_chunk(&chunk, &full, &[]);
            }
        }
        if name.id.is_higher_order()
            && let Some(v) = self.higher_order(recv, &name.text, &*args)?
        {
            return Ok(v);
        }
        builtin_method(recv, name, args)
    }
}

pub(super) fn as_closure(v: Option<&Value>) -> Result<Rc<super::value::ClosureData>> {
    match v {
        Some(Value::Closure(c)) => Ok(c.clone()),
        _ => bail!("this method expects a closure argument"),
    }
}

pub(super) fn option_inner(v: &Value) -> Option<Value> {
    match v {
        Value::Enum {
            enum_name,
            variant,
            data,
        } if &**enum_name == "Option" && &**variant == "Some" => {
            Some(data.first().cloned().unwrap_or(Value::Unit))
        }
        _ => None,
    }
}

/// A zero-argument closure that runs a constructor path like `Vec::new`, for
/// use as a value handed to `or_insert_with`.
// A path used as a function value, wrapped in a closure that forwards its
// `num_params` arguments to the path call. `num_params` is 0 for a constructor
// like `Vec::new` and 1 for a method reference or one-arg constructor.
pub(super) fn path_call_closure(segs: Vec<String>, num_params: usize) -> Value {
    let dst = num_params as u16;
    let mut chunk = Chunk::empty("<pathfn>");
    chunk.num_params = num_params;
    chunk.num_regs = num_params + 1;
    chunk.paths.push((segs, None));
    // Arguments land in registers 0..num_params, the result goes just past them.
    chunk.code.push(Op::CallPath {
        dst,
        path: 0,
        base: 0,
        argc: num_params as u16,
    });
    chunk.code.push(Op::Ret { src: dst });
    Value::Closure(Rc::new(ClosureData {
        chunk: Rc::new(chunk),
        captured: Vec::new(),
    }))
}

// `usize::MAX`, `i32::MIN` and friends. The 64 bit and wider limits are
// clamped to what an i64 value can hold, which is enough for sentinels and
// bounds. Returns None for anything that is not an integer limit path.
fn int_limit(ty: &str, name: &str) -> Option<Value> {
    // The float limits first, `f64::EPSILON` guards float comparisons.
    if ty == "f64" || ty == "f32" {
        let v = match (ty, name) {
            ("f64", "EPSILON") => f64::EPSILON,
            ("f32", "EPSILON") => f64::from(f32::EPSILON),
            ("f64", "MAX") => f64::MAX,
            ("f32", "MAX") => f64::from(f32::MAX),
            ("f64", "MIN") => f64::MIN,
            ("f32", "MIN") => f64::from(f32::MIN),
            (_, "INFINITY") => f64::INFINITY,
            (_, "NEG_INFINITY") => f64::NEG_INFINITY,
            (_, "NAN") => f64::NAN,
            _ => return None,
        };
        return Some(Value::Float(v));
    }
    let (min, max): (i64, i64) = match ty {
        "i8" => (i8::MIN as i64, i8::MAX as i64),
        "i16" => (i16::MIN as i64, i16::MAX as i64),
        "i32" => (i32::MIN as i64, i32::MAX as i64),
        "i64" | "i128" | "isize" => (i64::MIN, i64::MAX),
        "u8" => (0, u8::MAX as i64),
        "u16" => (0, u16::MAX as i64),
        "u32" => (0, u32::MAX as i64),
        "u64" | "u128" | "usize" => (0, i64::MAX),
        _ => return None,
    };
    match name {
        "MAX" => Some(Value::Int(max)),
        "MIN" => Some(Value::Int(min)),
        _ => None,
    }
}

pub(super) fn make_ordering(o: std::cmp::Ordering) -> Value {
    use std::cmp::Ordering::*;
    thread_local! {
        static VALUES: [Value; 3] = [
            ordering_value("Less"),
            ordering_value("Equal"),
            ordering_value("Greater"),
        ];
    }
    let index = match o {
        Less => 0,
        Equal => 1,
        Greater => 2,
    };
    VALUES.with(|values| values[index].clone())
}

fn ordering_value(variant: &str) -> Value {
    Value::Enum {
        enum_name: Rc::from("Ordering"),
        variant: Rc::from(variant),
        data: Value::empty_data(),
    }
}

pub(super) fn ordering_from_value(v: &Value) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering::*;
    match v {
        Value::Enum {
            enum_name, variant, ..
        } if &**enum_name == "Ordering" => match &**variant {
            "Less" => Some(Less),
            "Equal" => Some(Equal),
            "Greater" => Some(Greater),
            _ => None,
        },
        _ => None,
    }
}

// -- builtin methods -------------------------------------------------------

pub(super) fn builtin_method(
    recv: &Value,
    method: &MethodName,
    args: &mut [Value],
) -> Result<Value> {
    // Type tests apply to any receiver, so they are answered before the per
    // type dispatch below, which returns early and would never reach them.
    if let Some(v) = json_type_test(recv, method.text.as_str()) {
        return Ok(v);
    }
    // The hot receivers dispatch on the precompiled id, no string compares.
    match recv {
        Value::Str(s) => return str_method(s, method, &*args),
        Value::Vec(v) => return vec_method(v, method, args),
        Value::Map(m) => return map_method(m, method, args),
        _ => {}
    }
    let name = method.text.as_str();
    match recv {
        Value::Native(h) => {
            if let Some(v) = regex_native_method(h, name, &*args)? {
                return Ok(v);
            }
            if let Some(v) = super::crates_bridge::sha256_method(h, name, &*args)? {
                return Ok(v);
            }
            match super::native::native_method(h, name, args)? {
                Some(v) => Ok(v),
                None => generic_method(recv, name, &*args),
            }
        }
        Value::Int(_) | Value::Float(_) => num_method(recv, name, &*args),
        Value::Enum { enum_name, .. } if &**enum_name == "Option" => {
            opt_method(recv, method, &*args)
        }
        Value::Enum { enum_name, .. } if &**enum_name == "Result" => {
            res_method(recv, method, &*args)
        }
        Value::Struct(s) => match &**s.name() {
            "Duration" => duration_method(s, name),
            "Metadata" => metadata_method(s, name, &*args),
            "Permissions" => match name {
                "mode" => Ok(s.get("mode").unwrap_or(Value::Int(0))),
                "readonly" => Ok(s.get("readonly").unwrap_or(Value::Bool(false))),
                "set_readonly" => Ok(Value::Unit),
                _ => bail!("unknown method `{name}` on Permissions"),
            },
            "Command" => command_method(s, name, &*args),
            "ExitStatus" => exitstatus_method(s, name),
            "ReqwestClientBuilder"
            | "ReqwestRequest"
            | "ReqwestResponse"
            | "StatusCode"
            | "HeaderMap"
            | "HeaderValue" => http_method(s, name, &*args),
            "StdStream" => std_stream_method(s, name, args),
            "RegKey" => winreg_method(s, name, &*args),
            "ServiceManager" => manager_method(s, name, &*args),
            "Service" => service_method(s, name, &*args),
            "WmiConnection" => wmi_method(s, name, &*args),
            "Rng" => rng_method(name, &*args),
            "DateTime" => datetime_method(s, name, &*args),
            "Base64Engine" => base64_method(s, name, &*args),
            "Entry" => entry_method(s, name, &*args),
            "Element" => super::xmltree_bridge::element_method(s, name, &*args),
            "Child" => child_method(s, name, args),
            "Path" => path_method(s, name, &*args),
            "OsString" => os_string_method(s, name),
            "DirEntry" => dir_entry_method(s, name),
            "FileType" => file_type_method(s, name),
            "OpenOptions" => super::std_bridge::openoptions_method(s, name, &*args),
            _ => generic_method(recv, name, &*args),
        },
        _ => generic_method(recv, name, &*args),
    }
}
