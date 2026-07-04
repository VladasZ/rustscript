use std::rc::Rc;

use anyhow::{Result, bail};

use super::bytecode::{Chunk, MethodName, Op};

use super::value::{ClosureData, StructShape, Value};
use super::Interp;

use super::crates_bridge::*;
use super::http::*;
use super::jwt_bridge::*;
use super::methods::*;
use super::process::*;
use super::regex_bridge::*;
use super::std_bridge::*;


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
            if let Some(v) = self.unit_variant(None, name) {
                return Ok(v);
            }
            // A unit struct used as a value, `struct Marker;` then `Marker`.
            if let Some(def) = self.structs().get(name.as_str())
                && matches!(def.ast.fields, syn::Fields::Unit)
            {
                return Ok(Value::structure(StructShape::new(name.as_str(), Vec::new()), Vec::new()));
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
        // A bare constructor path used as a value, e.g. `Vec::new` handed to
        // `or_insert_with`. Wrap it in a zero-argument closure that calls it.
        if matches!(last.as_str(), "new" | "default") {
            return Ok(zero_arg_call_closure(segs.to_vec()));
        }
        bail!("unsupported path `{}`", segs.join("::"));
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
            if def.variants.iter().any(|v| {
                v.ident == variant && matches!(v.fields, syn::Fields::Unit)
            }) {
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
        // Threads run serially: spawn runs the closure now and hands back a
        // handle whose value is ready. Needs the interpreter to call it.
        if ns == "ctrlc" && last == "set_handler" {
            let closure = args.first().cloned().unwrap_or(Value::Unit);
            return Ok(match super::set_ctrlc_handler(closure) {
                Ok(()) => Value::ok(Value::Unit),
                Err(e) => Value::err(Value::str(e.to_string())),
            });
        }
        if ns == "thread" && last == "spawn" {
            let clo = as_closure(args.first())?;
            let result = self.call_closure(&clo, &[])?;
            return Ok(Value::struct_of("JoinHandle", [("result".into(), result)]));
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

    pub(super) fn eval_method(&self, recv: &Value, name: &MethodName, args: &mut [Value]) -> Result<Value> {
        // A method on a range acts on it as an iterator, so expand it to a Vec.
        let expanded;
        let recv = if matches!(recv, Value::Range { .. }) {
            expanded = Value::vec(self.into_iter_items(recv.clone())?);
            &expanded
        } else {
            recv
        };
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
        Value::Enum { enum_name, variant, data } if &**enum_name == "Option" && &**variant == "Some" => {
            Some(data.first().cloned().unwrap_or(Value::Unit))
        }
        _ => None,
    }
}

/// A zero-argument closure that runs a constructor path like `Vec::new`, for
/// use as a value handed to `or_insert_with`.
pub(super) fn zero_arg_call_closure(segs: Vec<String>) -> Value {
    let mut chunk = Chunk::empty("<ctor>");
    chunk.num_regs = 1;
    chunk.paths.push((segs, None));
    chunk.code.push(Op::CallPath { dst: 0, path: 0, base: 0, argc: 0 });
    chunk.code.push(Op::Ret { src: 0 });
    Value::Closure(Rc::new(ClosureData { chunk: Rc::new(chunk), captured: Vec::new() }))
}

// `usize::MAX`, `i32::MIN` and friends. The 64 bit and wider limits are
// clamped to what an i64 value can hold, which is enough for sentinels and
// bounds. Returns None for anything that is not an integer limit path.
fn int_limit(ty: &str, name: &str) -> Option<Value> {
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
    let variant = match o {
        Less => "Less",
        Equal => "Equal",
        Greater => "Greater",
    };
    Value::Enum {
        enum_name: "Ordering".into(),
        variant: variant.into(),
        data: Value::empty_data(),
    }
}

pub(super) fn ordering_from_value(v: &Value) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering::*;
    match v {
        Value::Enum { enum_name, variant, .. } if &**enum_name == "Ordering" => match &**variant {
            "Less" => Some(Less),
            "Equal" => Some(Equal),
            "Greater" => Some(Greater),
            _ => None,
        },
        _ => None,
    }
}


// -- builtin methods -------------------------------------------------------

pub(super) fn builtin_method(recv: &Value, method: &MethodName, args: &mut [Value]) -> Result<Value> {
    // The hot receivers dispatch on the precompiled id, no string compares.
    match recv {
        Value::Str(s) => return str_method(s, method, &*args),
        Value::Vec(v) => return vec_method(v, method, args),
        Value::Map(m) => return map_method(m, method, args),
        _ => {}
    }
    let name = method.text.as_str();
    match recv {
        Value::Native(h) => match super::native::native_method(h, name, args)? {
            Some(v) => Ok(v),
            None => generic_method(recv, name, &*args),
        },
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
            "HttpRequest" | "HttpResponse" | "HttpBody" | "StatusCode" => {
                http_method(s, name, &*args)
            }
            "StdStream" => std_stream_method(s, name, args),
            "Rng" => rng_method(name, &*args),
            "DateTime" => datetime_method(s, name, &*args),
            "Base64Engine" => base64_method(s, name, &*args),
            "Entry" => entry_method(s, name, &*args),
            "JoinHandle" => match name {
                "join" => Ok(Value::ok(s.get("result").unwrap_or(Value::Unit))),
                "is_finished" => Ok(Value::Bool(true)),
                _ => bail!("unknown method `{name}` on JoinHandle"),
            },
            "Child" => child_method(s, name, args),
            "Path" => path_method(s, name, &*args),
            "DirEntry" => dir_entry_method(s, name),
            "FileType" => file_type_method(s, name),
            "Regex" => regex_method(s, name, &*args),
            "Match" => match_method(s, name),
            "Captures" => captures_method(s, name, &*args),
            _ => generic_method(recv, name, &*args),
        },
        _ => generic_method(recv, name, &*args),
    }
}
