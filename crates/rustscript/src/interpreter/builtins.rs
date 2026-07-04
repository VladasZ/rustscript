use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use anyhow::{Result, anyhow, bail};

use super::bytecode::{Chunk, Op};
use super::native::{self, Native};
use super::value::{ClosureData, Fields, MapKey, Value};
use super::Interp;

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
        // A bare constructor path used as a value, e.g. `Vec::new` handed to
        // `or_insert_with`. Wrap it in a zero-argument closure that calls it.
        if matches!(last.as_str(), "new" | "default") {
            return Ok(zero_arg_call_closure(segs.to_vec()));
        }
        bail!("unsupported path `{}`", segs.join("::"));
    }

    fn unit_variant(&self, enum_name: Option<&str>, variant: &str) -> Option<Value> {
        for (name, def) in &self.enums {
            if let Some(want) = enum_name
                && want != name
            {
                continue;
            }
            if def.variants.iter().any(|v| {
                v.ident == variant && matches!(v.fields, syn::Fields::Unit)
            }) {
                return Some(Value::Enum {
                    enum_name: name.clone(),
                    variant: variant.to_string(),
                    data: Rc::new(RefCell::new(vec![])),
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
                _ => {}
            }
            if let Some(chunk) = self.user_function(name) {
                return self.run_chunk(&chunk, &args, &[]);
            }
            if self.structs().contains_key(name) {
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
            let mut f = Fields::new();
            f.insert("result".into(), result);
            return Ok(Value::Struct {
                name: "JoinHandle".into(),
                fields: Rc::new(RefCell::new(f)),
            });
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
    fn canonical(&self, segs: &[String]) -> Vec<String> {
        if let Some(full) = self.uses.get(&segs[0]) {
            let mut out = full.clone();
            out.extend_from_slice(&segs[1..]);
            out
        } else {
            segs.to_vec()
        }
    }

    fn make_tuple_struct(&self, name: &str, args: Vec<Value>) -> Result<Value> {
        let mut fields = Fields::new();
        for (i, v) in args.into_iter().enumerate() {
            fields.insert(i.to_string(), v);
        }
        Ok(Value::Struct {
            name: name.to_string(),
            fields: Rc::new(RefCell::new(fields)),
        })
    }

    fn make_tuple_variant(
        &self,
        enum_name: Option<&str>,
        variant: &str,
        args: &[Value],
    ) -> Option<Result<Value>> {
        for (name, def) in &self.enums {
            if let Some(want) = enum_name
                && want != name
            {
                continue;
            }
            if def.variants.iter().any(|v| v.ident == variant) {
                return Some(Ok(Value::Enum {
                    enum_name: name.clone(),
                    variant: variant.to_string(),
                    data: Rc::new(RefCell::new(args.to_vec())),
                }));
            }
        }
        None
    }

    pub(super) fn eval_method(&self, recv: Value, name: &str, args: Vec<Value>) -> Result<Value> {
        // A method on a range acts on it as an iterator, so expand it to a Vec.
        let recv = if matches!(recv, Value::Range { .. }) {
            Value::vec(self.into_iter_items(recv)?)
        } else {
            recv
        };
        let type_name = match &recv {
            Value::Struct { name, .. } => Some(name.clone()),
            Value::Enum { enum_name, .. } => Some(enum_name.clone()),
            _ => None,
        };
        if let Some(tn) = &type_name
            && let Some(chunk) = self.user_method(tn, name)
        {
            // The receiver is param 0, followed by the call arguments.
            let mut full = Vec::with_capacity(args.len() + 1);
            full.push(recv);
            full.extend(args);
            return self.run_chunk(&chunk, &full, &[]);
        }
        if let Some(v) = self.higher_order(&recv, name, &args)? {
            return Ok(v);
        }
        builtin_method(recv, name, args)
    }

    /// Methods that take a closure, on Vec, Option, and Result. Returns None
    /// when the method is not one of these, so plain dispatch can handle it.
    fn higher_order(&self, recv: &Value, name: &str, args: &[Value]) -> Result<Option<Value>> {
        match recv {
            Value::Vec(items) => self.vec_higher_order(items, name, args),
            Value::Enum { enum_name, variant, data } if enum_name == "Option" => {
                self.option_higher_order(variant, data, name, args)
            }
            Value::Enum { enum_name, variant, data } if enum_name == "Result" => {
                self.result_higher_order(variant, data, name, args)
            }
            Value::Struct { name: n, fields } if n == "Entry" => {
                self.entry_higher_order(fields, name, args)
            }
            _ => Ok(None),
        }
    }

    /// The closure forms of `HashMap::entry`: `or_insert_with`, `or_insert_with_key`,
    /// and `and_modify`. Non-closure forms fall through to `entry_method`.
    fn entry_higher_order(
        &self,
        fields: &Rc<RefCell<Fields>>,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let (map, key) = {
            let f = fields.borrow();
            let key = f
                .get("key")
                .and_then(|k| k.as_key())
                .ok_or_else(|| anyhow!("invalid entry key"))?;
            let Some(Value::Map(m)) = f.get("map") else {
                bail!("entry lost its map");
            };
            (m.clone(), key)
        };
        match name {
            "or_insert_with" | "or_insert_with_key" => {
                let present = map.borrow().contains_key(&key);
                if !present {
                    let clo = as_closure(args.first())?;
                    let call_args = if name == "or_insert_with_key" {
                        vec![key.to_value()]
                    } else {
                        vec![]
                    };
                    let v = self.call_closure(&clo, &call_args)?;
                    map.borrow_mut().insert(key.clone(), v);
                }
                Ok(Some(map.borrow().get(&key).cloned().unwrap_or(Value::Unit)))
            }
            "and_modify" => {
                if map.borrow().contains_key(&key) {
                    let clo = as_closure(args.first())?;
                    let current = map.borrow().get(&key).cloned().unwrap_or(Value::Unit);
                    let updated = self.call_closure(&clo, &[current])?;
                    // A closure that returns unit means it mutated in place via
                    // a shared container; otherwise take its return value.
                    if !matches!(updated, Value::Unit) {
                        map.borrow_mut().insert(key.clone(), updated);
                    }
                }
                // Return the Entry so further chaining (or_insert) still works.
                Ok(Some(Value::Struct {
                    name: "Entry".into(),
                    fields: fields.clone(),
                }))
            }
            _ => Ok(None),
        }
    }

    fn vec_higher_order(
        &self,
        items: &Rc<RefCell<Vec<Value>>>,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let clo = |i: usize| as_closure(args.get(i));
        let list = items.borrow().clone();
        let out = match name {
            "map" => {
                let f = clo(0)?;
                let mut r = Vec::with_capacity(list.len());
                for x in list {
                    r.push(self.call_closure(&f, &[x])?);
                }
                Value::vec(r)
            }
            "filter" => {
                let f = clo(0)?;
                let mut r = Vec::new();
                for x in list {
                    if self.call_closure(&f, &[x.clone()])?.is_truthy() {
                        r.push(x);
                    }
                }
                Value::vec(r)
            }
            "filter_map" => {
                let f = clo(0)?;
                let mut r = Vec::new();
                for x in list {
                    if let Some(inner) = option_inner(&self.call_closure(&f, &[x])?) {
                        r.push(inner);
                    }
                }
                Value::vec(r)
            }
            "flat_map" => {
                let f = clo(0)?;
                let mut r = Vec::new();
                for x in list {
                    r.extend(self.into_iter_items(self.call_closure(&f, &[x])?)?);
                }
                Value::vec(r)
            }
            "for_each" => {
                let f = clo(0)?;
                for x in list {
                    self.call_closure(&f, &[x])?;
                }
                Value::Unit
            }
            "find" => {
                let f = clo(0)?;
                let mut found = Value::none();
                for x in list {
                    if self.call_closure(&f, &[x.clone()])?.is_truthy() {
                        found = Value::some(x);
                        break;
                    }
                }
                found
            }
            "position" => {
                let f = clo(0)?;
                let mut found = Value::none();
                for (i, x) in list.into_iter().enumerate() {
                    if self.call_closure(&f, &[x])?.is_truthy() {
                        found = Value::some(Value::Int(i as i64));
                        break;
                    }
                }
                found
            }
            "any" => {
                let f = clo(0)?;
                let mut any = false;
                for x in list {
                    if self.call_closure(&f, &[x])?.is_truthy() {
                        any = true;
                        break;
                    }
                }
                Value::Bool(any)
            }
            "all" => {
                let f = clo(0)?;
                let mut all = true;
                for x in list {
                    if !self.call_closure(&f, &[x])?.is_truthy() {
                        all = false;
                        break;
                    }
                }
                Value::Bool(all)
            }
            "fold" => {
                let init = args.first().cloned().unwrap_or(Value::Unit);
                let f = clo(1)?;
                let mut acc = init;
                for x in list {
                    acc = self.call_closure(&f, &[acc, x])?;
                }
                acc
            }
            "reduce" => {
                let f = clo(0)?;
                let mut it = list.into_iter();
                match it.next() {
                    Some(first) => {
                        let mut acc = first;
                        for x in it {
                            acc = self.call_closure(&f, &[acc, x])?;
                        }
                        Value::some(acc)
                    }
                    None => Value::none(),
                }
            }
            "retain" => {
                let f = clo(0)?;
                let mut kept = Vec::new();
                for x in list {
                    if self.call_closure(&f, &[x.clone()])?.is_truthy() {
                        kept.push(x);
                    }
                }
                *items.borrow_mut() = kept;
                Value::Unit
            }
            "sort_by_key" => {
                let f = clo(0)?;
                let mut keyed = Vec::new();
                for x in list {
                    let k = self.call_closure(&f, &[x.clone()])?;
                    keyed.push((sort_key(&k), x));
                }
                keyed.sort_by(|a, b| a.0.cmp(&b.0));
                *items.borrow_mut() = keyed.into_iter().map(|(_, x)| x).collect();
                Value::Unit
            }
            "sort_by" => {
                let f = clo(0)?;
                let mut sorted = list;
                let mut err = None;
                sorted.sort_by(|a, b| {
                    if err.is_some() {
                        return std::cmp::Ordering::Equal;
                    }
                    match self.call_closure(&f, &[a.clone(), b.clone()]) {
                        Ok(v) => ordering_from_value(&v).unwrap_or(std::cmp::Ordering::Equal),
                        Err(e) => {
                            err = Some(e);
                            std::cmp::Ordering::Equal
                        }
                    }
                });
                if let Some(e) = err {
                    return Err(e);
                }
                *items.borrow_mut() = sorted;
                Value::Unit
            }
            "max_by_key" | "min_by_key" => {
                let f = clo(0)?;
                let want_max = name == "max_by_key";
                let mut best: Option<(SortKey, Value)> = None;
                for x in list {
                    let k = sort_key(&self.call_closure(&f, &[x.clone()])?);
                    let take = match &best {
                        None => true,
                        Some((bk, _)) => {
                            if want_max {
                                k >= *bk
                            } else {
                                k < *bk
                            }
                        }
                    };
                    if take {
                        best = Some((k, x));
                    }
                }
                match best {
                    Some((_, x)) => Value::some(x),
                    None => Value::none(),
                }
            }
            "take_while" => {
                let f = clo(0)?;
                let mut r = Vec::new();
                for x in list {
                    if self.call_closure(&f, &[x.clone()])?.is_truthy() {
                        r.push(x);
                    } else {
                        break;
                    }
                }
                Value::vec(r)
            }
            "skip_while" => {
                let f = clo(0)?;
                let mut r = Vec::new();
                let mut skipping = true;
                for x in list {
                    if skipping && self.call_closure(&f, &[x.clone()])?.is_truthy() {
                        continue;
                    }
                    skipping = false;
                    r.push(x);
                }
                Value::vec(r)
            }
            "partition" => {
                let f = clo(0)?;
                let (mut yes, mut no) = (Vec::new(), Vec::new());
                for x in list {
                    if self.call_closure(&f, &[x.clone()])?.is_truthy() {
                        yes.push(x);
                    } else {
                        no.push(x);
                    }
                }
                Value::Tuple(Rc::new(RefCell::new(vec![Value::vec(yes), Value::vec(no)])))
            }
            _ => return Ok(None),
        };
        Ok(Some(out))
    }

    fn option_higher_order(
        &self,
        variant: &str,
        data: &Rc<RefCell<Vec<Value>>>,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let is_some = variant == "Some";
        let inner = || data.borrow().first().cloned().unwrap_or(Value::Unit);
        let clo = |i: usize| as_closure(args.get(i));
        let out = match name {
            "map" => {
                if is_some {
                    Value::some(self.call_closure(&*clo(0)?, &[inner()])?)
                } else {
                    Value::none()
                }
            }
            "and_then" => {
                if is_some {
                    self.call_closure(&*clo(0)?, &[inner()])?
                } else {
                    Value::none()
                }
            }
            "filter" => {
                if is_some && self.call_closure(&*clo(0)?, &[inner()])?.is_truthy() {
                    Value::some(inner())
                } else {
                    Value::none()
                }
            }
            "map_or" => {
                let default = args.first().cloned().unwrap_or(Value::Unit);
                if is_some {
                    self.call_closure(&*clo(1)?, &[inner()])?
                } else {
                    default
                }
            }
            "unwrap_or_else" => {
                if is_some {
                    inner()
                } else {
                    self.call_closure(&*clo(0)?, &[])?
                }
            }
            "ok_or_else" => {
                if is_some {
                    Value::ok(inner())
                } else {
                    Value::err(self.call_closure(&*clo(0)?, &[])?)
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(out))
    }

    fn result_higher_order(
        &self,
        variant: &str,
        data: &Rc<RefCell<Vec<Value>>>,
        name: &str,
        args: &[Value],
    ) -> Result<Option<Value>> {
        let is_ok = variant == "Ok";
        let inner = || data.borrow().first().cloned().unwrap_or(Value::Unit);
        let clo = |i: usize| as_closure(args.get(i));
        let out = match name {
            "map" => {
                if is_ok {
                    Value::ok(self.call_closure(&*clo(0)?, &[inner()])?)
                } else {
                    Value::err(inner())
                }
            }
            "map_err" => {
                if is_ok {
                    Value::ok(inner())
                } else {
                    Value::err(self.call_closure(&*clo(0)?, &[inner()])?)
                }
            }
            "and_then" => {
                if is_ok {
                    self.call_closure(&*clo(0)?, &[inner()])?
                } else {
                    Value::err(inner())
                }
            }
            "unwrap_or_else" => {
                if is_ok {
                    inner()
                } else {
                    self.call_closure(&*clo(0)?, &[inner()])?
                }
            }
            "with_context" => {
                if is_ok {
                    Value::ok(inner())
                } else {
                    let ctx = self.call_closure(&*clo(0)?, &[])?.display();
                    Value::err(Value::str(format!("{ctx}\nCaused by: {}", inner().display())))
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(out))
    }
}

fn as_closure(v: Option<&Value>) -> Result<Rc<super::value::ClosureData>> {
    match v {
        Some(Value::Closure(c)) => Ok(c.clone()),
        _ => bail!("this method expects a closure argument"),
    }
}

fn option_inner(v: &Value) -> Option<Value> {
    match v {
        Value::Enum { enum_name, variant, data } if enum_name == "Option" && variant == "Some" => {
            Some(data.borrow().first().cloned().unwrap_or(Value::Unit))
        }
        _ => None,
    }
}

/// A zero-argument closure that runs a constructor path like `Vec::new`, for
/// use as a value handed to `or_insert_with`.
fn zero_arg_call_closure(segs: Vec<String>) -> Value {
    let mut chunk = Chunk::empty("<ctor>");
    chunk.num_regs = 1;
    chunk.paths.push((segs, None));
    chunk.code.push(Op::CallPath { dst: 0, path: 0, base: 0, argc: 0 });
    chunk.code.push(Op::Ret { src: 0 });
    Value::Closure(Rc::new(ClosureData { chunk: Rc::new(chunk), captured: Vec::new() }))
}

fn make_ordering(o: std::cmp::Ordering) -> Value {
    use std::cmp::Ordering::*;
    let variant = match o {
        Less => "Less",
        Equal => "Equal",
        Greater => "Greater",
    };
    Value::Enum {
        enum_name: "Ordering".into(),
        variant: variant.into(),
        data: Rc::new(RefCell::new(vec![])),
    }
}

fn ordering_from_value(v: &Value) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering::*;
    match v {
        Value::Enum { enum_name, variant, .. } if enum_name == "Ordering" => match variant.as_str() {
            "Less" => Some(Less),
            "Equal" => Some(Equal),
            "Greater" => Some(Greater),
            _ => None,
        },
        _ => None,
    }
}

// -- std bridges -----------------------------------------------------------

/// Native implementations of the supported subset of std and serde_json,
/// dispatched by the last two path segments as `module::func`. Returns None
/// when the namespace is not native, so callers can try other handlers.
fn native_call(module: &str, func: &str, args: &[Value]) -> Result<Option<Value>> {
    if module == "serde_json" {
        return bridge_serde_json(func, args).map(Some);
    }
    if module == "ureq" {
        return Ok(make_request(func, args));
    }
    let s = |i: usize| -> Result<String> {
        match args.get(i) {
            Some(v) => Ok(path_like(v)),
            None => bail!("missing argument {i} for {module}::{func}"),
        }
    };
    Ok(Some(match (module, func) {
            ("fs", "read_to_string") => wrap_io(std::fs::read_to_string(s(0)?)),
            ("fs", "read") => wrap_bytes(std::fs::read(s(0)?)),
            ("fs", "write") => wrap_unit(std::fs::write(s(0)?, s(1)?)),
            ("fs", "create_dir_all") => wrap_unit(std::fs::create_dir_all(s(0)?)),
            ("fs", "create_dir") => wrap_unit(std::fs::create_dir(s(0)?)),
            ("fs", "remove_file") => wrap_unit(std::fs::remove_file(s(0)?)),
            ("fs", "remove_dir_all") => wrap_unit(std::fs::remove_dir_all(s(0)?)),
            ("fs", "remove_dir") => wrap_unit(std::fs::remove_dir(s(0)?)),
            ("fs", "copy") => match std::fs::copy(s(0)?, s(1)?) {
                Ok(n) => Value::ok(Value::Int(n as i64)),
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            ("fs", "rename") => wrap_unit(std::fs::rename(s(0)?, s(1)?)),
            ("fs", "read_dir") => match std::fs::read_dir(s(0)?) {
                Ok(rd) => {
                    let mut items = Vec::new();
                    for e in rd {
                        match e {
                            Ok(entry) => items.push(Value::ok(make_dir_entry(&entry))),
                            Err(err) => items.push(Value::err(Value::str(err.to_string()))),
                        }
                    }
                    Value::ok(Value::vec(items))
                }
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            ("fs", "canonicalize") => match std::fs::canonicalize(s(0)?) {
                Ok(p) => Value::ok(make_path(p.display().to_string())),
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            ("env", "args") => {
                Value::vec(super::script_args().into_iter().map(Value::str).collect())
            }
            ("env", "var") => match std::env::var(s(0)?) {
                Ok(v) => Value::ok(Value::str(v)),
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            ("env", "current_dir") => match std::env::current_dir() {
                Ok(p) => Value::ok(make_path(p.display().to_string())),
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            ("env", "set_var") => {
                // Safety: single threaded interpreter.
                unsafe { std::env::set_var(s(0)?, s(1)?) };
                Value::Unit
            }
            ("env", "remove_var") => {
                unsafe { std::env::remove_var(s(0)?) };
                Value::Unit
            }
            ("env", "var_os") => match std::env::var_os(s(0)?) {
                Some(v) => Value::some(Value::str(v.to_string_lossy().into_owned())),
                None => Value::none(),
            },
            ("env", "vars") | ("env", "vars_os") => Value::vec(
                std::env::vars()
                    .map(|(k, v)| Value::Tuple(Rc::new(RefCell::new(vec![Value::str(k), Value::str(v)]))))
                    .collect(),
            ),
            ("env", "set_current_dir") => wrap_unit(std::env::set_current_dir(s(0)?)),
            ("env", "temp_dir") => make_path(std::env::temp_dir().display().to_string()),
            ("process", "exit") => {
                let code = args.first().and_then(as_i64).unwrap_or(0) as i32;
                std::process::exit(code);
            }
            ("process", "abort") => std::process::abort(),
            // -- io -------------------------------------------------------
            ("io", "stdin") => make_std_stream(
                "stdin",
                Native::Reader(std::io::BufReader::new(Box::new(std::io::stdin()))),
            ),
            ("io", "stdout") => {
                make_std_stream("stdout", Native::Writer(Box::new(std::io::stdout())))
            }
            ("io", "stderr") => {
                make_std_stream("stderr", Native::Writer(Box::new(std::io::stderr())))
            }
            // -- fs metadata & links -------------------------------------
            ("fs", "metadata") => match std::fs::metadata(s(0)?) {
                Ok(m) => Value::ok(make_metadata(&m)),
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            ("fs", "symlink_metadata") => match std::fs::symlink_metadata(s(0)?) {
                Ok(m) => Value::ok(make_metadata(&m)),
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            ("fs", "read_link") => match std::fs::read_link(s(0)?) {
                Ok(p) => Value::ok(make_path(p.display().to_string())),
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            ("fs", "hard_link") => wrap_unit(std::fs::hard_link(s(0)?, s(1)?)),
            ("fs", "symlink") => wrap_unit(make_symlink(&s(0)?, &s(1)?)),
            // -- thread ---------------------------------------------------
            ("thread", "sleep") => {
                if let Some(d) = args.first().and_then(duration_from_value) {
                    std::thread::sleep(d);
                }
                Value::Unit
            }
            _ => return crate_bridge(module, func, args),
        }))
    }

/// The interpreter has no real threads, so a symlink helper picks the right
/// platform call. On Windows a file vs dir symlink needs distinct functions;
/// we treat the target kind by whether the source exists as a directory.
fn make_symlink(src: &str, dst: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(src, dst)
    }
    #[cfg(windows)]
    {
        if std::path::Path::new(src).is_dir() {
            std::os::windows::fs::symlink_dir(src, dst)
        } else {
            std::os::windows::fs::symlink_file(src, dst)
        }
    }
}

fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Int(i) => Some(*i),
        _ => None,
    }
}

/// Turn a value into a path string. A `Path`/`PathBuf` value carries the path in
/// its `s` field; anything else uses its display form.
pub(super) fn path_like(v: &Value) -> String {
    match v {
        Value::Str(s) => s.borrow().clone(),
        Value::Struct { name, fields } if name == "Path" || name == "PathBuf" => fields
            .borrow()
            .get("s")
            .map(|s| s.display())
            .unwrap_or_default(),
        other => other.display(),
    }
}

/// Wrap a std stream handle so `is_terminal` can name its stream while reads
/// and writes delegate to the inner native handle.
fn make_std_stream(kind: &str, inner: Native) -> Value {
    let mut f = Fields::new();
    f.insert("kind".into(), Value::str(kind));
    f.insert("inner".into(), inner.wrap());
    Value::Struct {
        name: "StdStream".into(),
        fields: Rc::new(RefCell::new(f)),
    }
}

fn std_stream_method(fields: &Rc<RefCell<Fields>>, name: &str, args: &[Value]) -> Result<Value> {
    use std::io::IsTerminal;
    if name == "is_terminal" {
        let kind = fields.borrow().get("kind").map(|v| v.display()).unwrap_or_default();
        let tty = match kind.as_str() {
            "stdin" => std::io::stdin().is_terminal(),
            "stderr" => std::io::stderr().is_terminal(),
            _ => std::io::stdout().is_terminal(),
        };
        return Ok(Value::Bool(tty));
    }
    if matches!(name, "lock" | "by_ref") {
        return Ok(Value::Struct {
            name: "StdStream".into(),
            fields: fields.clone(),
        });
    }
    let inner = match fields.borrow().get("inner") {
        Some(Value::Native(h)) => h.clone(),
        _ => bail!("std stream lost its handle"),
    };
    match native::native_method(&inner, name, args)? {
        Some(v) => Ok(v),
        None => bail!("unknown method `{name}` on a std stream"),
    }
}

/// Turn a script `Duration` value into a real `std::time::Duration`.
pub(super) fn duration_from_value(v: &Value) -> Option<std::time::Duration> {
    if let Value::Struct { name, fields } = v
        && name == "Duration"
    {
        let f = fields.borrow();
        let secs = field_int(&f, "secs") as u64;
        let nanos = field_int(&f, "nanos") as u32;
        return Some(std::time::Duration::new(secs, nanos));
    }
    None
}

/// Build a real `Command` from a script `Command` value's fields.
fn build_command(f: &Fields) -> std::process::Command {
    let program = f.get("program").map(|v| v.display()).unwrap_or_default();
    let mut cmd = std::process::Command::new(&program);
    if let Some(Value::Vec(a)) = f.get("args") {
        for item in a.borrow().iter() {
            cmd.arg(item.display());
        }
    }
    if let Some(cwd) = f.get("cwd") {
        cmd.current_dir(cwd.display());
    }
    if let Some(Value::Map(envs)) = f.get("envs") {
        for (k, v) in envs.borrow().iter() {
            cmd.env(k.to_value().display(), v.display());
        }
    }
    cmd
}

/// Run a `Command` value once it has been fully built, returning an `Output`.
fn run_command(fields: &Rc<RefCell<Fields>>) -> Value {
    let f = fields.borrow();
    match build_command(&f).output() {
        Ok(out) => Value::ok(make_output(out)),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

/// Map a stored `Stdio` marker to a real `std::process::Stdio`, defaulting to
/// inherit so a spawned child shares the terminal like a shell command.
fn stdio_for(f: &Fields, key: &str) -> std::process::Stdio {
    match f.get(key) {
        Some(Value::Struct { name, fields }) if name == "Stdio" => {
            match fields.borrow().get("kind").map(|v| v.display()).as_deref() {
                Some("piped") => std::process::Stdio::piped(),
                Some("null") => std::process::Stdio::null(),
                _ => std::process::Stdio::inherit(),
            }
        }
        _ => std::process::Stdio::inherit(),
    }
}

/// Spawn a `Command`, returning a `Child` value whose stdin/stdout/stderr
/// fields hold the piped ends as native handles.
fn spawn_command(fields: &Rc<RefCell<Fields>>) -> Value {
    let f = fields.borrow();
    let mut cmd = build_command(&f);
    cmd.stdin(stdio_for(&f, "stdin"));
    cmd.stdout(stdio_for(&f, "stdout"));
    cmd.stderr(stdio_for(&f, "stderr"));
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Value::err(Value::str(e.to_string())),
    };
    let stdin = child
        .stdin
        .take()
        .map(|w| Native::ChildStdin(w).wrap())
        .map(Value::some)
        .unwrap_or_else(Value::none);
    let stdout = child
        .stdout
        .take()
        .map(|r| Native::Reader(std::io::BufReader::new(Box::new(r) as Box<dyn std::io::Read>)).wrap())
        .map(Value::some)
        .unwrap_or_else(Value::none);
    let stderr = child
        .stderr
        .take()
        .map(|r| Native::Reader(std::io::BufReader::new(Box::new(r) as Box<dyn std::io::Read>)).wrap())
        .map(Value::some)
        .unwrap_or_else(Value::none);
    let mut cf = Fields::new();
    cf.insert("handle".into(), Native::Child(child).wrap());
    cf.insert("stdin".into(), stdin);
    cf.insert("stdout".into(), stdout);
    cf.insert("stderr".into(), stderr);
    Value::ok(Value::Struct {
        name: "Child".into(),
        fields: Rc::new(RefCell::new(cf)),
    })
}

/// Build an `ExitStatus` value with `code` and `success`.
pub(super) fn make_exit_status(status: std::process::ExitStatus) -> Value {
    let mut st = Fields::new();
    st.insert("code".into(), Value::Int(status.code().unwrap_or(-1) as i64));
    st.insert("success".into(), Value::Bool(status.success()));
    Value::Struct {
        name: "ExitStatus".into(),
        fields: Rc::new(RefCell::new(st)),
    }
}

/// Build an `Output` value with `stdout`, `stderr`, and `status`.
pub(super) fn make_output(out: std::process::Output) -> Value {
    let mut o = Fields::new();
    o.insert(
        "stdout".into(),
        Value::str(String::from_utf8_lossy(&out.stdout).into_owned()),
    );
    o.insert(
        "stderr".into(),
        Value::str(String::from_utf8_lossy(&out.stderr).into_owned()),
    );
    o.insert("status".into(), make_exit_status(out.status));
    Value::Struct {
        name: "Output".into(),
        fields: Rc::new(RefCell::new(o)),
    }
}

/// Build a `Duration` value carrying whole and sub-second parts.
pub(super) fn make_duration(d: std::time::Duration) -> Value {
    let mut f = Fields::new();
    f.insert("secs".into(), Value::Int(d.as_secs() as i64));
    f.insert("nanos".into(), Value::Int(d.subsec_nanos() as i64));
    Value::Struct {
        name: "Duration".into(),
        fields: Rc::new(RefCell::new(f)),
    }
}

/// Build a `Metadata` value with the common accessors materialized as fields.
/// The Unix `MetadataExt` fields are gated so the interpreter still builds on
/// Windows, where a script would use different accessors.
pub(super) fn make_metadata(m: &std::fs::Metadata) -> Value {
    let mut f = Fields::new();
    f.insert("len".into(), Value::Int(m.len() as i64));
    f.insert("is_dir".into(), Value::Bool(m.is_dir()));
    f.insert("is_file".into(), Value::Bool(m.is_file()));
    f.insert("is_symlink".into(), Value::Bool(m.is_symlink()));
    f.insert("readonly".into(), Value::Bool(m.permissions().readonly()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;
        f.insert("mode".into(), Value::Int(m.permissions().mode() as i64));
        f.insert("dev".into(), Value::Int(m.dev() as i64));
        f.insert("ino".into(), Value::Int(m.ino() as i64));
        f.insert("uid".into(), Value::Int(m.uid() as i64));
        f.insert("gid".into(), Value::Int(m.gid() as i64));
        f.insert("mtime".into(), Value::Int(m.mtime() as i64));
    }
    if let Ok(t) = m.modified() {
        f.insert("modified".into(), super::native::Native::SystemTime(t).wrap());
    }
    Value::Struct {
        name: "Metadata".into(),
        fields: Rc::new(RefCell::new(f)),
    }
}

// -- ureq http bridge ------------------------------------------------------

/// Build an `HttpRequest` value for `ureq::get`, `ureq::post`, and friends.
/// `ureq::agent()` instead builds a cookie-persisting agent handle.
fn make_request(func: &str, args: &[Value]) -> Option<Value> {
    if func == "agent" {
        return Some(Native::Agent(ureq::agent()).wrap());
    }
    let method = http_verb(func)?;
    Some(build_http_request(method, args.first(), None))
}

fn http_verb(func: &str) -> Option<&'static str> {
    Some(match func {
        "get" => "GET",
        "post" => "POST",
        "put" => "PUT",
        "delete" => "DELETE",
        "patch" => "PATCH",
        "head" => "HEAD",
        _ => return None,
    })
}

/// Build an `HttpRequest`, optionally bound to an agent so its cookie jar and
/// config carry through the call.
pub(super) fn build_http_request(method: &str, url: Option<&Value>, agent: Option<Value>) -> Value {
    let mut fields = Fields::new();
    fields.insert("method".into(), Value::str(method));
    fields.insert(
        "url".into(),
        Value::str(url.map(|v| v.display()).unwrap_or_default()),
    );
    fields.insert("headers".into(), Value::vec(vec![]));
    if let Some(a) = agent {
        fields.insert("agent".into(), a);
    }
    Value::Struct {
        name: "HttpRequest".into(),
        fields: Rc::new(RefCell::new(fields)),
    }
}

fn http_method(
    struct_name: &str,
    fields: &Rc<RefCell<Fields>>,
    method: &str,
    args: &[Value],
) -> Result<Value> {
    match struct_name {
        "HttpRequest" => request_method(fields, method, args),
        "HttpResponse" => Ok(response_method(fields, method)),
        "HttpBody" => body_method(fields, method),
        "StatusCode" => Ok(status_method(fields, method)),
        _ => bail!("unknown http method `{method}`"),
    }
}

fn request_method(
    fields: &Rc<RefCell<Fields>>,
    method: &str,
    args: &[Value],
) -> Result<Value> {
    let this = || Value::Struct {
        name: "HttpRequest".into(),
        fields: fields.clone(),
    };
    match method {
        "header" | "set" | "content_type" => {
            let pair = if method == "content_type" {
                vec![Value::str("Content-Type"), args.first().cloned().unwrap_or(Value::Unit)]
            } else {
                vec![
                    args.first().cloned().unwrap_or(Value::Unit),
                    args.get(1).cloned().unwrap_or(Value::Unit),
                ]
            };
            if let Some(Value::Vec(h)) = fields.borrow().get("headers") {
                h.borrow_mut().push(Value::Tuple(Rc::new(RefCell::new(pair))));
            }
            Ok(this())
        }
        "call" => Ok(run_request(fields, None)),
        "send" | "send_string" => {
            let body = args.first().map(|v| v.display()).unwrap_or_default();
            Ok(run_request(fields, Some(body)))
        }
        "send_json" => {
            let json = value_to_json(args.first().unwrap_or(&Value::Unit))?;
            if let Some(Value::Vec(h)) = fields.borrow().get("headers") {
                h.borrow_mut().push(Value::Tuple(Rc::new(RefCell::new(vec![
                    Value::str("Content-Type"),
                    Value::str("application/json"),
                ]))));
            }
            Ok(run_request(fields, Some(serde_json::to_string(&json)?)))
        }
        "query" => {
            let pair = vec![
                args.first().cloned().unwrap_or(Value::Unit),
                args.get(1).cloned().unwrap_or(Value::Unit),
            ];
            let mut f = fields.borrow_mut();
            let entry = f.entry("query".into()).or_insert_with(|| Value::vec(vec![]));
            if let Value::Vec(q) = entry {
                q.borrow_mut().push(Value::Tuple(Rc::new(RefCell::new(pair))));
            }
            drop(f);
            Ok(this())
        }
        // ureq 3 sets timeouts through `.config().timeout_global(Some(d)).build()`.
        // `config` and `build` are pass-throughs; the timeout is stored for the call.
        "config" | "build" => Ok(this()),
        "timeout" | "timeout_global" | "timeout_connect" => {
            // The argument may be a bare Duration or an Option<Duration>.
            let dur = match args.first() {
                Some(Value::Enum { data, .. }) => data.borrow().first().cloned(),
                other => other.cloned(),
            };
            if let Some(d) = dur {
                fields.borrow_mut().insert("timeout".into(), d);
            }
            Ok(this())
        }
        _ => bail!("unknown method `{method}` on a request"),
    }
}

/// Percent-encode a query value the simple way, enough for API params.
fn encode_query(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn run_request(fields: &Rc<RefCell<Fields>>, body: Option<String>) -> Value {
    let f = fields.borrow();
    let verb = f.get("method").map(|v| v.display()).unwrap_or_else(|| "GET".into());
    let mut url = f.get("url").map(|v| v.display()).unwrap_or_default();
    // Append any query parameters onto the URL.
    if let Some(Value::Vec(q)) = f.get("query") {
        let q = q.borrow();
        if !q.is_empty() {
            let sep = if url.contains('?') { '&' } else { '?' };
            url.push(sep);
            let parts: Vec<String> = q
                .iter()
                .filter_map(|item| {
                    if let Value::Tuple(pair) = item {
                        let pair = pair.borrow();
                        Some(format!(
                            "{}={}",
                            encode_query(&pair[0].display()),
                            encode_query(&pair[1].display())
                        ))
                    } else {
                        None
                    }
                })
                .collect();
            url.push_str(&parts.join("&"));
        }
    }
    let timeout = f.get("timeout").and_then(duration_from_value);
    let agent = match f.get("agent") {
        Some(Value::Native(h)) => Some(h.clone()),
        _ => None,
    };
    let mut headers = Vec::new();
    if let Some(Value::Vec(h)) = f.get("headers") {
        for item in h.borrow().iter() {
            if let Value::Tuple(pair) = item {
                let pair = pair.borrow();
                headers.push((pair[0].display(), pair[1].display()));
            }
        }
    }
    match do_http(&verb, &url, &headers, body, timeout, agent.as_ref()) {
        Ok((status, text)) => {
            let mut rf = Fields::new();
            rf.insert("status".into(), Value::Int(status as i64));
            rf.insert("body".into(), Value::str(text));
            Value::ok(Value::Struct {
                name: "HttpResponse".into(),
                fields: Rc::new(RefCell::new(rf)),
            })
        }
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

fn do_http(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<String>,
    timeout: Option<std::time::Duration>,
    agent: Option<&Rc<RefCell<Native>>>,
) -> Result<(u16, String)> {
    // Build the request through the shared agent when one is given, so its
    // cookie jar carries across calls; otherwise use ureq's free functions.
    let agent = agent.and_then(|h| match &*h.borrow() {
        Native::Agent(a) => Some(a.clone()),
        _ => None,
    });
    let with_body = matches!(method, "POST" | "PUT" | "PATCH");
    if with_body {
        let mut b = match (&agent, method) {
            (Some(a), "POST") => a.post(url),
            (Some(a), "PUT") => a.put(url),
            (Some(a), _) => a.patch(url),
            (None, "POST") => ureq::post(url),
            (None, "PUT") => ureq::put(url),
            (None, _) => ureq::patch(url),
        };
        if let Some(d) = timeout {
            b = b.config().timeout_global(Some(d)).build();
        }
        for (k, v) in headers {
            b = b.header(k, v);
        }
        let mut resp = b.send(body.as_deref().unwrap_or(""))?;
        Ok((resp.status().as_u16(), resp.body_mut().read_to_string()?))
    } else {
        let mut b = match (&agent, method) {
            (Some(a), "DELETE") => a.delete(url),
            (Some(a), "HEAD") => a.head(url),
            (Some(a), _) => a.get(url),
            (None, "DELETE") => ureq::delete(url),
            (None, "HEAD") => ureq::head(url),
            (None, _) => ureq::get(url),
        };
        if let Some(d) = timeout {
            b = b.config().timeout_global(Some(d)).build();
        }
        for (k, v) in headers {
            b = b.header(k, v);
        }
        let mut resp = b.call()?;
        Ok((resp.status().as_u16(), resp.body_mut().read_to_string()?))
    }
}

fn response_method(fields: &Rc<RefCell<Fields>>, method: &str) -> Value {
    let f = fields.borrow();
    match method {
        "status" => {
            let mut sf = Fields::new();
            sf.insert("code".into(), f.get("status").cloned().unwrap_or(Value::Int(0)));
            Value::Struct {
                name: "StatusCode".into(),
                fields: Rc::new(RefCell::new(sf)),
            }
        }
        "body_mut" | "body" | "into_body" => {
            let mut bf = Fields::new();
            bf.insert("text".into(), f.get("body").cloned().unwrap_or_else(|| Value::str("")));
            Value::Struct {
                name: "HttpBody".into(),
                fields: Rc::new(RefCell::new(bf)),
            }
        }
        "into_string" => Value::ok(f.get("body").cloned().unwrap_or_else(|| Value::str(""))),
        _ => Value::Unit,
    }
}

fn body_method(fields: &Rc<RefCell<Fields>>, method: &str) -> Result<Value> {
    let text = fields.borrow().get("text").map(|v| v.display()).unwrap_or_default();
    Ok(match method {
        "read_to_string" => Value::ok(Value::str(text)),
        "read_json" => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(j) => Value::ok(json_to_value(&j)),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        _ => bail!("unknown method `{method}` on a body"),
    })
}

fn status_method(fields: &Rc<RefCell<Fields>>, method: &str) -> Value {
    let code = match fields.borrow().get("code") {
        Some(Value::Int(c)) => *c,
        _ => 0,
    };
    match method {
        "as_u16" | "as_int" => Value::Int(code),
        "is_success" => Value::Bool((200..300).contains(&code)),
        "is_client_error" => Value::Bool((400..500).contains(&code)),
        "is_server_error" => Value::Bool((500..600).contains(&code)),
        _ => Value::Unit,
    }
}

// -- path, directory entry, and file type ----------------------------------

pub(super) fn make_path(s: impl Into<String>) -> Value {
    let mut f = Fields::new();
    f.insert("s".into(), Value::str(s.into()));
    Value::Struct {
        name: "Path".into(),
        fields: Rc::new(RefCell::new(f)),
    }
}

fn make_dir_entry(entry: &std::fs::DirEntry) -> Value {
    let mut f = Fields::new();
    f.insert("path".into(), Value::str(entry.path().display().to_string()));
    f.insert(
        "name".into(),
        Value::str(entry.file_name().to_string_lossy().into_owned()),
    );
    Value::Struct {
        name: "DirEntry".into(),
        fields: Rc::new(RefCell::new(f)),
    }
}

fn make_file_type(path: &std::path::Path) -> Value {
    let mut f = Fields::new();
    f.insert("is_dir".into(), Value::Bool(path.is_dir()));
    f.insert("is_file".into(), Value::Bool(path.is_file()));
    f.insert(
        "is_symlink".into(),
        Value::Bool(path.symlink_metadata().map(|m| m.file_type().is_symlink()).unwrap_or(false)),
    );
    Value::Struct {
        name: "FileType".into(),
        fields: Rc::new(RefCell::new(f)),
    }
}

fn path_string(fields: &Rc<RefCell<Fields>>, key: &str) -> String {
    fields.borrow().get(key).map(|v| v.display()).unwrap_or_default()
}

fn path_method(
    fields: &Rc<RefCell<Fields>>,
    method: &str,
    args: &[Value],
) -> Result<Value> {
    let s = path_string(fields, "s");
    let p = std::path::Path::new(&s);
    let opt_str = |o: Option<&std::ffi::OsStr>| match o {
        Some(v) => Value::some(Value::str(v.to_string_lossy().into_owned())),
        None => Value::none(),
    };
    Ok(match method {
        "display" | "to_string_lossy" => Value::str(s.clone()),
        "to_str" => Value::some(Value::str(s.clone())),
        "into_string" | "into_os_string" => Value::ok(Value::str(s.clone())),
        "to_owned" | "to_path_buf" | "clone" | "as_path" | "as_os_str" => make_path(s.clone()),
        "is_dir" => Value::Bool(p.is_dir()),
        "is_file" => Value::Bool(p.is_file()),
        "is_absolute" => Value::Bool(p.is_absolute()),
        "exists" => Value::Bool(p.exists()),
        "file_name" => match p.file_name() {
            Some(n) => make_path(n.to_string_lossy().into_owned()),
            None => Value::none(),
        },
        "file_stem" => opt_str(p.file_stem()),
        "extension" => opt_str(p.extension()),
        "parent" => match p.parent() {
            Some(par) => Value::some(make_path(par.display().to_string())),
            None => Value::none(),
        },
        "join" | "push" => {
            let joined = p.join(args.first().map(|v| v.display()).unwrap_or_default());
            make_path(joined.display().to_string())
        }
        _ => bail!("unknown method `{method}` on Path"),
    })
}

fn dir_entry_method(
    fields: &Rc<RefCell<Fields>>,
    method: &str,
) -> Result<Value> {
    let path = path_string(fields, "path");
    Ok(match method {
        "path" => make_path(path),
        "file_name" => make_path(path_string(fields, "name")),
        "file_type" => Value::ok(make_file_type(std::path::Path::new(&path))),
        _ => bail!("unknown method `{method}` on DirEntry"),
    })
}

fn file_type_method(fields: &Rc<RefCell<Fields>>, method: &str) -> Result<Value> {
    let get = |k: &str| fields.borrow().get(k).cloned().unwrap_or(Value::Bool(false));
    Ok(match method {
        "is_dir" => get("is_dir"),
        "is_file" => get("is_file"),
        "is_symlink" => get("is_symlink"),
        _ => bail!("unknown method `{method}` on FileType"),
    })
}

// -- regex bridge ----------------------------------------------------------

fn make_regex(pattern: String) -> Value {
    let mut f = Fields::new();
    f.insert("pattern".into(), Value::str(pattern));
    Value::Struct {
        name: "Regex".into(),
        fields: Rc::new(RefCell::new(f)),
    }
}

fn make_match(m: &regex::Match) -> Value {
    let mut f = Fields::new();
    f.insert("text".into(), Value::str(m.as_str().to_string()));
    f.insert("start".into(), Value::Int(m.start() as i64));
    f.insert("end".into(), Value::Int(m.end() as i64));
    Value::Struct {
        name: "Match".into(),
        fields: Rc::new(RefCell::new(f)),
    }
}

fn make_captures(re: &regex::Regex, caps: &regex::Captures) -> Value {
    let groups: Vec<Value> = (0..caps.len())
        .map(|i| match caps.get(i) {
            Some(m) => Value::some(make_match(&m)),
            None => Value::none(),
        })
        .collect();
    let mut names = BTreeMap::new();
    for (i, name) in re.capture_names().enumerate() {
        if let Some(n) = name {
            names.insert(MapKey::Str(n.to_string()), Value::Int(i as i64));
        }
    }
    let mut f = Fields::new();
    f.insert("groups".into(), Value::vec(groups));
    f.insert("names".into(), Value::Map(Rc::new(RefCell::new(names))));
    Value::Struct {
        name: "Captures".into(),
        fields: Rc::new(RefCell::new(f)),
    }
}

fn regex_method(
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

fn match_method(fields: &Rc<RefCell<Fields>>, method: &str) -> Result<Value> {
    let f = fields.borrow();
    Ok(match method {
        "as_str" => f.get("text").cloned().unwrap_or_else(|| Value::str("")),
        "start" => f.get("start").cloned().unwrap_or(Value::Int(0)),
        "end" => f.get("end").cloned().unwrap_or(Value::Int(0)),
        _ => bail!("unknown method `{method}` on Match"),
    })
}

fn captures_method(
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

fn capture_group(fields: &Rc<RefCell<Fields>>, i: usize) -> Value {
    match fields.borrow().get("groups") {
        Some(Value::Vec(g)) => g.borrow().get(i).cloned().unwrap_or_else(Value::none),
        _ => Value::none(),
    }
}

fn capture_name_index(fields: &Rc<RefCell<Fields>>, name: &str) -> Option<usize> {
    if let Some(Value::Map(names)) = fields.borrow().get("names")
        && let Some(Value::Int(i)) = names.borrow().get(&MapKey::Str(name.to_string()))
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
        Value::Str(s) => capture_name_index(fields, &s.borrow())
            .ok_or_else(|| anyhow!("no capture group named `{}`", s.borrow()))?,
        _ => bail!("invalid capture index"),
    };
    match capture_group(fields, idx) {
        Value::Enum { variant, data, .. } if variant == "Some" => {
            let m = data.borrow().first().cloned().unwrap_or(Value::Unit);
            if let Value::Struct { fields: mf, .. } = m {
                return Ok(mf.borrow().get("text").cloned().unwrap_or_else(|| Value::str("")));
            }
            bail!("bad capture group")
        }
        _ => bail!("no match for capture group {idx}"),
    }
}

fn wrap_io(r: std::io::Result<String>) -> Value {
    match r {
        Ok(s) => Value::ok(Value::str(s)),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

fn wrap_bytes(r: std::io::Result<Vec<u8>>) -> Value {
    match r {
        Ok(bytes) => Value::ok(Value::vec(bytes.into_iter().map(|b| Value::Int(b as i64)).collect())),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

fn wrap_unit(r: std::io::Result<()>) -> Value {
    match r {
        Ok(()) => Value::ok(Value::Unit),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

fn one(args: Vec<Value>) -> Result<Value> {
    args.into_iter()
        .next()
        .ok_or_else(|| anyhow!("expected one argument"))
}

/// Associated functions like `String::new`, `Vec::new`, `HashMap::new`.
fn assoc_fn(ty: &str, func: &str, args: &[Value]) -> Result<Option<Value>> {
    Ok(Some(match (ty, func) {
        ("String", "new") | ("String", "with_capacity") => Value::str(""),
        ("String", "from") => Value::str(args.first().map(|v| v.display()).unwrap_or_default()),
        ("String", "from_utf8_lossy") => Value::str(bytes_to_string(args.first())),
        ("String", "from_utf8") => Value::ok(Value::str(bytes_to_string(args.first()))),
        ("Command", "new") => {
            let mut fields = Fields::new();
            fields.insert(
                "program".into(),
                args.first().cloned().unwrap_or_else(|| Value::str("")),
            );
            fields.insert("args".into(), Value::vec(vec![]));
            Value::Struct {
                name: "Command".into(),
                fields: Rc::new(RefCell::new(fields)),
            }
        }
        ("Vec", "new") | ("Vec", "with_capacity") => Value::vec(vec![]),
        ("Vec", "from") => match args.first() {
            Some(Value::Vec(v)) => Value::vec(v.borrow().clone()),
            Some(other) => Value::vec(vec![other.clone()]),
            None => Value::vec(vec![]),
        },
        ("HashMap", "new") | ("BTreeMap", "new") | ("HashMap", "with_capacity")
        | ("HashSet", "new") | ("BTreeSet", "new") => {
            Value::Map(Rc::new(RefCell::new(BTreeMap::new())))
        }
        ("Box" | "Rc" | "Arc" | "RefCell" | "Cell", "new") => {
            args.first().cloned().unwrap_or(Value::Unit)
        }
        // Our file and pipe readers are already buffered, so wrapping is a
        // pass-through; a raw socket is turned into a buffered reader.
        ("BufReader" | "BufWriter", "new" | "with_capacity") => {
            match args.last() {
                Some(Value::Native(h)) if matches!(&*h.borrow(), Native::Stream(_)) => {
                    let Native::Stream(s) = &*h.borrow() else {
                        unreachable!()
                    };
                    match s.try_clone() {
                        Ok(clone) => Native::Reader(std::io::BufReader::new(
                            Box::new(clone) as Box<dyn std::io::Read>,
                        ))
                        .wrap(),
                        Err(e) => return Err(anyhow!("cannot buffer socket: {e}")),
                    }
                }
                other => other.cloned().unwrap_or(Value::Unit),
            }
        }
        ("PathBuf", "new") => make_path(""),
        ("PathBuf" | "Path", "from") => {
            make_path(args.first().map(|v| v.display()).unwrap_or_default())
        }
        ("Path", "new") => make_path(args.first().map(|v| v.display()).unwrap_or_default()),
        ("Regex", "new") => {
            let pat = args.first().map(|v| v.display()).unwrap_or_default();
            match regex::Regex::new(&pat) {
                Ok(_) => Value::ok(make_regex(pat)),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        ("Some", _) => Value::some(args.first().cloned().unwrap_or(Value::Unit)),
        ("Option", "Some") => Value::some(args.first().cloned().unwrap_or(Value::Unit)),
        ("Result", "Ok") => Value::ok(args.first().cloned().unwrap_or(Value::Unit)),
        ("Result", "Err") => Value::err(args.first().cloned().unwrap_or(Value::Unit)),
        // -- files -----------------------------------------------------
        ("File", "open") => open_file(&arg_str(args, 0), std::fs::OpenOptions::new().read(true)),
        ("File", "create") => open_file(
            &arg_str(args, 0),
            std::fs::OpenOptions::new().write(true).create(true).truncate(true),
        ),
        ("File", "create_new") => open_file(
            &arg_str(args, 0),
            std::fs::OpenOptions::new().write(true).create_new(true),
        ),
        ("OpenOptions", "new") => {
            let mut f = Fields::new();
            for k in ["read", "write", "append", "create", "create_new", "truncate"] {
                f.insert(k.into(), Value::Bool(false));
            }
            Value::Struct {
                name: "OpenOptions".into(),
                fields: Rc::new(RefCell::new(f)),
            }
        }
        // -- time ------------------------------------------------------
        ("Instant", "now") => Native::Instant(std::time::Instant::now()).wrap(),
        ("SystemTime", "now") => Native::SystemTime(std::time::SystemTime::now()).wrap(),
        ("Duration", "from_secs") => make_duration(std::time::Duration::from_secs(
            arg_int(args, 0) as u64,
        )),
        ("Duration", "from_millis") => make_duration(std::time::Duration::from_millis(
            arg_int(args, 0) as u64,
        )),
        ("Duration", "from_micros") => make_duration(std::time::Duration::from_micros(
            arg_int(args, 0) as u64,
        )),
        ("Duration", "from_nanos") => make_duration(std::time::Duration::from_nanos(
            arg_int(args, 0) as u64,
        )),
        ("Duration", "new") => make_duration(std::time::Duration::new(
            arg_int(args, 0) as u64,
            arg_int(args, 1) as u32,
        )),
        // -- net -------------------------------------------------------
        ("TcpListener", "bind") => match std::net::TcpListener::bind(arg_str(args, 0)) {
            Ok(l) => Value::ok(Native::Listener(l).wrap()),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        ("TcpStream", "connect") => match std::net::TcpStream::connect(arg_str(args, 0)) {
            Ok(s) => Value::ok(Native::Stream(s).wrap()),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        ("SeekFrom", "Start" | "End" | "Current") => Value::Enum {
            enum_name: "SeekFrom".into(),
            variant: func.to_string(),
            data: Rc::new(RefCell::new(vec![args.first().cloned().unwrap_or(Value::Int(0))])),
        },
        ("Agent", "new_with_defaults") => Native::Agent(ureq::agent()).wrap(),
        ("Stdio", "piped") | ("Stdio", "inherit") | ("Stdio", "null") => {
            let mut f = Fields::new();
            f.insert("kind".into(), Value::str(func));
            Value::Struct {
                name: "Stdio".into(),
                fields: Rc::new(RefCell::new(f)),
            }
        }
        _ => return Ok(None),
    }))
}

fn arg_str(args: &[Value], i: usize) -> String {
    args.get(i).map(path_like).unwrap_or_default()
}

fn arg_int(args: &[Value], i: usize) -> i64 {
    match args.get(i) {
        Some(Value::Int(n)) => *n,
        _ => 0,
    }
}

fn open_file(path: &str, opts: &std::fs::OpenOptions) -> Value {
    match opts.open(path) {
        Ok(f) => Value::ok(Native::File(std::io::BufReader::new(f)).wrap()),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

/// Bridges for the extra crates a script may `use`. Reached when a
/// `module::func` call is not a plain std bridge.
fn crate_bridge(module: &str, func: &str, args: &[Value]) -> Result<Option<Value>> {
    let s0 = || args.first().map(|v| v.display()).unwrap_or_default();
    Ok(Some(match (module, func) {
        // dirs -------------------------------------------------------------
        ("dirs", "home_dir") => opt_path(dirs::home_dir()),
        ("dirs", "cache_dir") => opt_path(dirs::cache_dir()),
        ("dirs", "config_dir") => opt_path(dirs::config_dir()),
        ("dirs", "config_local_dir") => opt_path(dirs::config_local_dir()),
        ("dirs", "data_dir") => opt_path(dirs::data_dir()),
        ("dirs", "data_local_dir") => opt_path(dirs::data_local_dir()),
        ("dirs", "executable_dir") => opt_path(dirs::executable_dir()),
        ("dirs", "runtime_dir") => opt_path(dirs::runtime_dir()),
        ("dirs", "desktop_dir") => opt_path(dirs::desktop_dir()),
        ("dirs", "download_dir") => opt_path(dirs::download_dir()),
        ("dirs", "document_dir") => opt_path(dirs::document_dir()),
        // which ------------------------------------------------------------
        ("which", "which") => match which::which(s0()) {
            Ok(p) => Value::ok(make_path(p.display().to_string())),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        // glob -------------------------------------------------------------
        ("glob", "glob") => match glob::glob(&s0()) {
            Ok(paths) => Value::ok(Value::vec(
                paths
                    .map(|r| match r {
                        Ok(p) => Value::ok(make_path(p.display().to_string())),
                        Err(e) => Value::err(Value::str(e.to_string())),
                    })
                    .collect(),
            )),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        // hex --------------------------------------------------------------
        ("hex", "encode") => Value::str(hex::encode(bytes_arg(args.first()))),
        ("hex", "decode") => match hex::decode(s0()) {
            Ok(b) => Value::ok(bytes_to_vec(&b)),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        // toml -------------------------------------------------------------
        ("toml", "from_str") => match toml::from_str::<serde_json::Value>(&s0()) {
            Ok(j) => Value::ok(json_to_value(&j)),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        ("toml", "to_string") | ("toml", "to_string_pretty") => {
            match toml::to_string(&value_to_json(args.first().unwrap_or(&Value::Unit))?) {
                Ok(s) => Value::ok(Value::str(s)),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        // serde_yaml -------------------------------------------------------
        ("serde_yaml", "from_str") => match serde_yaml::from_str::<serde_json::Value>(&s0()) {
            Ok(j) => Value::ok(json_to_value(&j)),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        ("serde_yaml", "to_string") => {
            match serde_yaml::to_string(&value_to_json(args.first().unwrap_or(&Value::Unit))?) {
                Ok(s) => Value::ok(Value::str(s)),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        // rand -------------------------------------------------------------
        ("rand", "rng") | ("rand", "thread_rng") => Value::Struct {
            name: "Rng".into(),
            fields: Rc::new(RefCell::new(Fields::new())),
        },
        ("rand", "random") => Value::Float(rand::random::<f64>()),
        // chrono -----------------------------------------------------------
        ("Utc", "now") | ("Local", "now") => now_datetime(module == "Local"),
        // tempfile ---------------------------------------------------------
        ("tempfile", "tempdir") => match tempfile::tempdir() {
            Ok(d) => Value::ok(Native::TempDir(d).wrap()),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        ("tempfile", "tempfile") => match tempfile::tempfile() {
            Ok(f) => Value::ok(Native::File(std::io::BufReader::new(f)).wrap()),
            Err(e) => Value::err(Value::str(e.to_string())),
        },
        _ => return Ok(None),
    }))
}

/// Recognize a base64 engine constant name and build a marker value carrying
/// which alphabet it uses, so `.encode`/`.decode` can pick the right engine.
fn base64_engine(name: &str) -> Option<Value> {
    let kind = match name {
        "STANDARD" | "BASE64_STANDARD" => "standard",
        "STANDARD_NO_PAD" | "BASE64_STANDARD_NO_PAD" => "standard_no_pad",
        "URL_SAFE" | "BASE64_URL_SAFE" => "url_safe",
        "URL_SAFE_NO_PAD" | "BASE64_URL_SAFE_NO_PAD" => "url_safe_no_pad",
        _ => return None,
    };
    let mut f = Fields::new();
    f.insert("kind".into(), Value::str(kind));
    Some(Value::Struct {
        name: "Base64Engine".into(),
        fields: Rc::new(RefCell::new(f)),
    })
}

fn base64_method(fields: &Rc<RefCell<Fields>>, method: &str, args: &[Value]) -> Result<Value> {
    use base64::Engine;
    use base64::engine::general_purpose::{
        STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD,
    };
    let kind = fields.borrow().get("kind").map(|v| v.display()).unwrap_or_default();
    macro_rules! pick {
        ($m:ident, $($a:tt)*) => {
            match kind.as_str() {
                "standard_no_pad" => STANDARD_NO_PAD.$m($($a)*),
                "url_safe" => URL_SAFE.$m($($a)*),
                "url_safe_no_pad" => URL_SAFE_NO_PAD.$m($($a)*),
                _ => STANDARD.$m($($a)*),
            }
        };
    }
    Ok(match method {
        "encode" => Value::str(pick!(encode, bytes_arg(args.first()))),
        "decode" => {
            let input = args.first().map(|v| v.display()).unwrap_or_default();
            match pick!(decode, &input) {
                Ok(b) => Value::ok(bytes_to_vec(&b)),
                Err(e) => Value::err(Value::str(e.to_string())),
            }
        }
        _ => bail!("unknown method `{method}` on a base64 engine"),
    })
}

/// Build a `DateTime` value for `Utc::now()` / `Local::now()`, storing the unix
/// timestamp so `format` can reconstruct a real chrono value.
fn now_datetime(local: bool) -> Value {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let mut f = Fields::new();
    f.insert("secs".into(), Value::Int(now.as_secs() as i64));
    f.insert("nanos".into(), Value::Int(now.subsec_nanos() as i64));
    f.insert("local".into(), Value::Bool(local));
    Value::Struct {
        name: "DateTime".into(),
        fields: Rc::new(RefCell::new(f)),
    }
}

fn datetime_method(fields: &Rc<RefCell<Fields>>, name: &str, args: &[Value]) -> Result<Value> {
    use chrono::{DateTime, Local, Utc};
    let f = fields.borrow();
    let secs = field_int(&f, "secs") as i64;
    let nanos = field_int(&f, "nanos") as u32;
    let local = matches!(f.get("local"), Some(Value::Bool(true)));
    let utc: DateTime<Utc> = DateTime::from_timestamp(secs, nanos).unwrap_or_default();
    Ok(match name {
        "timestamp" => Value::Int(secs as i64),
        "timestamp_millis" => Value::Int(secs as i64 * 1000 + (nanos / 1_000_000) as i64),
        "to_rfc3339" => Value::str(utc.to_rfc3339()),
        "format" => {
            let fmt = args.first().map(|v| v.display()).unwrap_or_default();
            if local {
                Value::str(utc.with_timezone(&Local).format(&fmt).to_string())
            } else {
                Value::str(utc.format(&fmt).to_string())
            }
        }
        "year" => Value::Int(chrono::Datelike::year(&utc) as i64),
        "month" => Value::Int(chrono::Datelike::month(&utc) as i64),
        "day" => Value::Int(chrono::Datelike::day(&utc) as i64),
        "hour" => Value::Int(chrono::Timelike::hour(&utc) as i64),
        "minute" => Value::Int(chrono::Timelike::minute(&utc) as i64),
        "second" => Value::Int(chrono::Timelike::second(&utc) as i64),
        _ => bail!("unknown method `{name}` on DateTime"),
    })
}

fn rng_method(name: &str, args: &[Value]) -> Result<Value> {
    use rand::RngExt;
    let mut rng = rand::rng();
    Ok(match name {
        "random_range" | "gen_range" => match args.first() {
            Some(Value::Range { start, end, inclusive }) => {
                let hi = if *inclusive { end + 1 } else { *end };
                if hi > *start {
                    Value::Int(rng.random_range(*start..hi))
                } else {
                    Value::Int(*start)
                }
            }
            _ => bail!("random_range needs a range"),
        },
        "random_bool" | "gen_bool" => {
            let p = match args.first() {
                Some(Value::Float(f)) => *f,
                Some(Value::Int(i)) => *i as f64,
                _ => 0.5,
            };
            Value::Bool(rng.random_bool(p.clamp(0.0, 1.0)))
        }
        "random" | "r#gen" | "gen" => Value::Float(rng.random::<f64>()),
        "fill_bytes" | "fill" => {
            if let Some(Value::Vec(v)) = args.first() {
                let mut buf = v.borrow_mut();
                for slot in buf.iter_mut() {
                    *slot = Value::Int(rng.random::<u8>() as i64);
                }
            }
            Value::Unit
        }
        _ => bail!("unknown method `{name}` on Rng"),
    })
}

fn opt_path(p: Option<std::path::PathBuf>) -> Value {
    match p {
        Some(p) => Value::some(make_path(p.display().to_string())),
        None => Value::none(),
    }
}

fn bytes_arg(v: Option<&Value>) -> Vec<u8> {
    match v {
        Some(Value::Str(s)) => s.borrow().clone().into_bytes(),
        Some(Value::Vec(items)) => items
            .borrow()
            .iter()
            .filter_map(|x| match x {
                Value::Int(i) => Some(*i as u8),
                _ => None,
            })
            .collect(),
        Some(other) => other.display().into_bytes(),
        None => Vec::new(),
    }
}

fn bytes_to_vec(b: &[u8]) -> Value {
    Value::vec(b.iter().map(|x| Value::Int(*x as i64)).collect())
}

// -- serde_json bridge -----------------------------------------------------

fn bridge_serde_json(func: &str, args: &[Value]) -> Result<Value> {
    match func {
        "from_str" => {
            let s = match args.first() {
                Some(Value::Str(s)) => s.borrow().clone(),
                Some(other) => other.display(),
                None => bail!("from_str needs a string"),
            };
            match serde_json::from_str::<serde_json::Value>(&s) {
                Ok(j) => Ok(Value::ok(json_to_value(&j))),
                Err(e) => Ok(Value::err(Value::str(e.to_string()))),
            }
        }
        "to_string" | "to_string_pretty" => {
            let v = args.first().cloned().unwrap_or(Value::Unit);
            let j = value_to_json(&v)?;
            let s = if func == "to_string_pretty" {
                serde_json::to_string_pretty(&j)?
            } else {
                serde_json::to_string(&j)?
            };
            Ok(Value::ok(Value::str(s)))
        }
        other => bail!("unsupported serde_json function `{other}`"),
    }
}

fn json_to_value(j: &serde_json::Value) -> Value {
    match j {
        serde_json::Value::Null => Value::none(),
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i as i64)
            } else {
                Value::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Value::str(s.clone()),
        serde_json::Value::Array(a) => Value::vec(a.iter().map(json_to_value).collect()),
        serde_json::Value::Object(o) => {
            let mut map = BTreeMap::new();
            for (k, v) in o {
                map.insert(MapKey::Str(k.clone()), json_to_value(v));
            }
            Value::Map(Rc::new(RefCell::new(map)))
        }
    }
}

fn value_to_json(v: &Value) -> Result<serde_json::Value> {
    use serde_json::Value as J;
    Ok(match v {
        Value::Unit => J::Null,
        Value::Bool(b) => J::Bool(*b),
        Value::Int(i) => J::Number(serde_json::Number::from(*i as i64)),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(J::Number)
            .unwrap_or(J::Null),
        Value::Char(c) => J::String(c.to_string()),
        Value::Str(s) => J::String(s.borrow().clone()),
        Value::Vec(items) | Value::Tuple(items) => {
            J::Array(items.borrow().iter().map(value_to_json).collect::<Result<_>>()?)
        }
        Value::Map(map) => {
            let mut obj = serde_json::Map::new();
            for (k, val) in map.borrow().iter() {
                obj.insert(k.to_value().display(), value_to_json(val)?);
            }
            J::Object(obj)
        }
        Value::Struct { fields, .. } => {
            let mut obj = serde_json::Map::new();
            for (k, val) in fields.borrow().iter() {
                obj.insert(k.clone(), value_to_json(val)?);
            }
            J::Object(obj)
        }
        Value::Enum {
            enum_name,
            variant,
            data,
        } => {
            if enum_name == "Option" {
                match variant.as_str() {
                    "Some" => value_to_json(&data.borrow()[0])?,
                    _ => J::Null,
                }
            } else {
                let data = data.borrow();
                if data.is_empty() {
                    J::String(variant.clone())
                } else {
                    let mut obj = serde_json::Map::new();
                    obj.insert(
                        variant.clone(),
                        J::Array(data.iter().map(value_to_json).collect::<Result<_>>()?),
                    );
                    J::Object(obj)
                }
            }
        }
        Value::Range { .. } => bail!("cannot serialize a range to json"),
        Value::Closure(_) => bail!("cannot serialize a closure to json"),
        Value::Native(n) => bail!("cannot serialize a {} to json", n.borrow().type_name()),
    })
}

// -- builtin methods -------------------------------------------------------

fn builtin_method(recv: Value, name: &str, args: Vec<Value>) -> Result<Value> {
    match &recv {
        Value::Native(h) => match super::native::native_method(h, name, &args)? {
            Some(v) => Ok(v),
            None => generic_method(&recv, name, &args),
        },
        Value::Struct { name: n, fields } if n == "Duration" => duration_method(fields, name),
        Value::Struct { name: n, fields } if n == "Metadata" => {
            metadata_method(fields, name, &args)
        }
        Value::Struct { name: n, fields } if n == "Permissions" => {
            let f = fields.borrow();
            match name {
                "mode" => Ok(f.get("mode").cloned().unwrap_or(Value::Int(0))),
                "readonly" => Ok(f.get("readonly").cloned().unwrap_or(Value::Bool(false))),
                "set_readonly" => Ok(Value::Unit),
                _ => bail!("unknown method `{name}` on Permissions"),
            }
        }
        Value::Str(s) => str_method(s, name, &args),
        Value::Vec(v) => vec_method(v, name, &args),
        Value::Map(m) => map_method(m, name, &args),
        Value::Int(_) | Value::Float(_) => num_method(&recv, name, &args),
        Value::Enum { enum_name, .. } if enum_name == "Option" => opt_method(&recv, name, &args),
        Value::Enum { enum_name, .. } if enum_name == "Result" => res_method(&recv, name, &args),
        Value::Struct { name: n, fields } if n == "Command" => {
            command_method(fields, name, &args)
        }
        Value::Struct { name: n, fields } if n == "ExitStatus" => {
            exitstatus_method(fields, name)
        }
        Value::Struct { name: n, fields }
            if matches!(
                n.as_str(),
                "HttpRequest" | "HttpResponse" | "HttpBody" | "StatusCode"
            ) =>
        {
            http_method(n, fields, name, &args)
        }
        Value::Struct { name: n, fields } if n == "StdStream" => {
            std_stream_method(fields, name, &args)
        }
        Value::Struct { name: n, .. } if n == "Rng" => rng_method(name, &args),
        Value::Struct { name: n, fields } if n == "DateTime" => {
            datetime_method(fields, name, &args)
        }
        Value::Struct { name: n, fields } if n == "Base64Engine" => {
            base64_method(fields, name, &args)
        }
        Value::Struct { name: n, fields } if n == "Entry" => entry_method(fields, name, &args),
        Value::Struct { name: n, fields } if n == "JoinHandle" => {
            let f = fields.borrow();
            match name {
                "join" => Ok(Value::ok(f.get("result").cloned().unwrap_or(Value::Unit))),
                "is_finished" => Ok(Value::Bool(true)),
                _ => bail!("unknown method `{name}` on JoinHandle"),
            }
        }
        Value::Struct { name: n, fields } if n == "Child" => child_method(fields, name, &args),
        Value::Struct { name: n, fields } if n == "Path" => path_method(fields, name, &args),
        Value::Struct { name: n, fields } if n == "DirEntry" => dir_entry_method(fields, name),
        Value::Struct { name: n, fields } if n == "FileType" => file_type_method(fields, name),
        Value::Struct { name: n, fields } if n == "Regex" => regex_method(fields, name, &args),
        Value::Struct { name: n, fields } if n == "Match" => match_method(fields, name),
        Value::Struct { name: n, fields } if n == "Captures" => {
            captures_method(fields, name, &args)
        }
        _ => generic_method(&recv, name, &args),
    }
}

fn command_method(
    fields: &Rc<RefCell<Fields>>,
    name: &str,
    args: &[Value],
) -> Result<Value> {
    let cmd_value = || Value::Struct {
        name: "Command".into(),
        fields: fields.clone(),
    };
    Ok(match name {
        "arg" => {
            if let Some(Value::Vec(list)) = fields.borrow().get("args") {
                list.borrow_mut()
                    .push(args.first().cloned().unwrap_or(Value::Unit));
            }
            cmd_value()
        }
        "args" => {
            if let (Some(Value::Vec(list)), Some(Value::Vec(extra))) =
                (fields.borrow().get("args"), args.first())
            {
                list.borrow_mut().extend(extra.borrow().iter().cloned());
            }
            cmd_value()
        }
        "current_dir" => {
            fields
                .borrow_mut()
                .insert("cwd".into(), args.first().cloned().unwrap_or(Value::Unit));
            cmd_value()
        }
        "env" => {
            let mut f = fields.borrow_mut();
            let key = args.first().map(|v| v.display()).unwrap_or_default();
            let val = args.get(1).cloned().unwrap_or(Value::Unit);
            let entry = f
                .entry("envs".into())
                .or_insert_with(|| Value::Map(Rc::new(RefCell::new(BTreeMap::new()))));
            if let Value::Map(m) = entry {
                m.borrow_mut().insert(MapKey::Str(key), val);
            }
            drop(f);
            cmd_value()
        }
        "stdin" | "stdout" | "stderr" => {
            fields
                .borrow_mut()
                .insert(name.into(), args.first().cloned().unwrap_or(Value::Unit));
            cmd_value()
        }
        "spawn" => return Ok(spawn_command(fields)),
        "output" => run_command(fields),
        "status" => match run_command(fields) {
            Value::Enum { data, .. } => {
                let out = data.borrow().first().cloned().unwrap_or(Value::Unit);
                match out {
                    Value::Struct { fields: of, .. } => {
                        Value::ok(of.borrow().get("status").cloned().unwrap_or(Value::Unit))
                    }
                    other => Value::ok(other),
                }
            }
            other => other,
        },
        _ => bail!("unknown method `{name}` on Command"),
    })
}

/// Methods on a spawned `Child`. Lifecycle calls delegate to the real child
/// handle; `wait_with_output` reads any piped stdout/stderr to the end first.
/// Drop the real `ChildStdin` inside a shared handle, closing the pipe. Walks a
/// `Some(Native)` wrapper from `child.stdin.take()`.
fn close_child_stdin(v: &Value) {
    match v {
        Value::Native(rc) => *rc.borrow_mut() = Native::Closed,
        Value::Enum { enum_name, variant, data }
            if enum_name == "Option" && variant == "Some" =>
        {
            if let Some(inner) = data.borrow().first() {
                close_child_stdin(inner);
            }
        }
        _ => {}
    }
}

fn child_method(fields: &Rc<RefCell<Fields>>, name: &str, args: &[Value]) -> Result<Value> {
    // Waiting on a child that was fed piped stdin must first close that pipe,
    // or the child blocks forever on EOF. Real Rust closes it when the taken
    // `ChildStdin` drops. The VM keeps every value alive in a register for the
    // whole call, so a `let w = cat.stdin.take()` clone stays live and the
    // writer never drops on its own. Close it through the shared handle instead,
    // which drops the real `ChildStdin` no matter how many clones exist.
    if matches!(name, "wait" | "wait_with_output") {
        let stdin_val = fields.borrow().get("stdin").cloned();
        if let Some(v) = stdin_val {
            close_child_stdin(&v);
        }
        if let Some(slot) = fields.borrow_mut().get_mut("stdin") {
            *slot = Value::none();
        }
    }
    if name == "wait_with_output" {
        let out = drain_child_pipe(fields, "stdout");
        let err = drain_child_pipe(fields, "stderr");
        let status = {
            let handle = child_handle(fields)?;
            let mut h = handle.borrow_mut();
            if let Native::Child(c) = &mut *h {
                match c.wait() {
                    Ok(s) => s,
                    Err(e) => return Ok(Value::err(Value::str(e.to_string()))),
                }
            } else {
                bail!("child handle missing");
            }
        };
        let mut o = Fields::new();
        o.insert("stdout".into(), Value::str(out));
        o.insert("stderr".into(), Value::str(err));
        o.insert("status".into(), make_exit_status(status));
        return Ok(Value::ok(Value::Struct {
            name: "Output".into(),
            fields: Rc::new(RefCell::new(o)),
        }));
    }
    let handle = child_handle(fields)?;
    match native::native_method(&handle, name, args)? {
        Some(v) => Ok(v),
        None => bail!("unknown method `{name}` on Child"),
    }
}

fn child_handle(fields: &Rc<RefCell<Fields>>) -> Result<Rc<RefCell<Native>>> {
    match fields.borrow().get("handle") {
        Some(Value::Native(h)) => Ok(h.clone()),
        _ => bail!("child handle missing"),
    }
}

/// Read a child's piped stdout/stderr field to the end as a string.
fn drain_child_pipe(fields: &Rc<RefCell<Fields>>, key: &str) -> String {
    let handle = match fields.borrow().get(key) {
        Some(Value::Enum { data, .. }) => match data.borrow().first() {
            Some(Value::Native(h)) => h.clone(),
            _ => return String::new(),
        },
        _ => return String::new(),
    };
    let target = Value::str("");
    match native::native_method(&handle, "read_to_string", std::slice::from_ref(&target)) {
        Ok(_) => {}
        Err(_) => return String::new(),
    }
    if let Value::Str(s) = &target {
        s.borrow().clone()
    } else {
        String::new()
    }
}

/// The `HashMap::entry` slot, without closures. Returns the stored value; for
/// container values that Rc-share, mutating the result mutates the map, so
/// `map.entry(k).or_insert_with(Vec::new).push(x)` accumulates in place.
fn entry_method(fields: &Rc<RefCell<Fields>>, name: &str, args: &[Value]) -> Result<Value> {
    let f = fields.borrow();
    let key = f
        .get("key")
        .and_then(|k| k.as_key())
        .ok_or_else(|| anyhow!("invalid entry key"))?;
    let Some(Value::Map(m)) = f.get("map") else {
        bail!("entry lost its map");
    };
    Ok(match name {
        "or_insert" => {
            let default = args.first().cloned().unwrap_or(Value::Unit);
            let mut map = m.borrow_mut();
            map.entry(key).or_insert(default).clone()
        }
        "or_default" => {
            let mut map = m.borrow_mut();
            map.entry(key).or_insert(Value::Int(0)).clone()
        }
        "key" => key.to_value(),
        _ => bail!("unknown method `{name}` on Entry"),
    })
}

fn exitstatus_method(fields: &Rc<RefCell<Fields>>, name: &str) -> Result<Value> {
    let f = fields.borrow();
    Ok(match name {
        "success" => f.get("success").cloned().unwrap_or(Value::Bool(false)),
        "code" => match f.get("code") {
            Some(v) => Value::some(v.clone()),
            None => Value::none(),
        },
        _ => bail!("unknown method `{name}` on ExitStatus"),
    })
}

fn duration_method(fields: &Rc<RefCell<Fields>>, name: &str) -> Result<Value> {
    let f = fields.borrow();
    let secs = field_int(&f, "secs") as u64;
    let nanos = field_int(&f, "nanos") as u32;
    let total_nanos = secs as u128 * 1_000_000_000 + nanos as u128;
    Ok(match name {
        "as_secs" => Value::Int(secs as i64),
        "as_millis" => Value::Int((total_nanos / 1_000_000) as i64),
        "as_micros" => Value::Int((total_nanos / 1_000) as i64),
        "as_nanos" => Value::Int(total_nanos as i64),
        "subsec_nanos" => Value::Int(nanos as i64),
        "subsec_millis" => Value::Int((nanos / 1_000_000) as i64),
        "subsec_micros" => Value::Int((nanos / 1_000) as i64),
        "as_secs_f64" => Value::Float(secs as f64 + nanos as f64 / 1e9),
        "is_zero" => Value::Bool(total_nanos == 0),
        _ => bail!("unknown method `{name}` on Duration"),
    })
}

fn metadata_method(fields: &Rc<RefCell<Fields>>, name: &str, _args: &[Value]) -> Result<Value> {
    let f = fields.borrow();
    let get = |k: &str| f.get(k).cloned().unwrap_or(Value::Unit);
    Ok(match name {
        "len" => get("len"),
        "is_dir" => get("is_dir"),
        "is_file" => get("is_file"),
        "is_symlink" => get("is_symlink"),
        "modified" | "created" | "accessed" => match f.get("modified") {
            Some(v) => Value::ok(v.clone()),
            None => Value::err(Value::str("timestamp not available".to_string())),
        },
        "mode" | "dev" | "ino" | "uid" | "gid" | "mtime" => get(name),
        "permissions" => {
            let mut p = Fields::new();
            p.insert("mode".into(), get("mode"));
            p.insert("readonly".into(), get("readonly"));
            Value::Struct {
                name: "Permissions".into(),
                fields: Rc::new(RefCell::new(p)),
            }
        }
        _ => bail!("unknown method `{name}` on Metadata"),
    })
}

fn field_int(f: &Fields, k: &str) -> i64 {
    match f.get(k) {
        Some(Value::Int(i)) => *i,
        _ => 0,
    }
}

fn bytes_to_string(arg: Option<&Value>) -> String {
    match arg {
        Some(Value::Str(s)) => s.borrow().clone(),
        Some(Value::Vec(v)) => {
            let bytes: Vec<u8> = v
                .borrow()
                .iter()
                .filter_map(|x| match x {
                    Value::Int(i) => Some(*i as u8),
                    _ => None,
                })
                .collect();
            String::from_utf8_lossy(&bytes).into_owned()
        }
        _ => String::new(),
    }
}

fn generic_method(recv: &Value, name: &str, _args: &[Value]) -> Result<Value> {
    match (recv, name) {
        (_, "clone") => Ok(recv.clone()),
        (_, "to_string") => Ok(Value::str(recv.display())),
        (Value::Bool(b), "as_bool") => Ok(Value::some(Value::Bool(*b))),
        (Value::Bool(b), "then_some") => Ok(if *b { Value::some(Value::Unit) } else { Value::none() }),
        (Value::Vec(v), "as_array") => Ok(Value::some(Value::vec(v.borrow().clone()))),
        _ => bail!("unknown method `{name}` on {}", recv.type_name()),
    }
}

fn str_method(s: &Rc<RefCell<String>>, name: &str, args: &[Value]) -> Result<Value> {
    let arg_str = |i: usize| -> String {
        args.get(i).map(|v| v.display()).unwrap_or_default()
    };
    Ok(match name {
        "len" => Value::Int(s.borrow().len() as i64),
        "is_empty" => Value::Bool(s.borrow().is_empty()),
        "clone" | "to_string" | "to_owned" | "trim_string" => Value::str(s.borrow().clone()),
        "to_uppercase" | "to_ascii_uppercase" => Value::str(s.borrow().to_uppercase()),
        "to_lowercase" | "to_ascii_lowercase" => Value::str(s.borrow().to_lowercase()),
        "trim" => Value::str(s.borrow().trim().to_string()),
        "trim_start" => Value::str(s.borrow().trim_start().to_string()),
        "trim_end" => Value::str(s.borrow().trim_end().to_string()),
        "push" => {
            if let Some(Value::Char(c)) = args.first() {
                s.borrow_mut().push(*c);
            }
            Value::Unit
        }
        "push_str" => {
            s.borrow_mut().push_str(&arg_str(0));
            Value::Unit
        }
        "contains" => Value::Bool(s.borrow().contains(&arg_str(0))),
        "starts_with" => Value::Bool(s.borrow().starts_with(&arg_str(0))),
        "ends_with" => Value::Bool(s.borrow().ends_with(&arg_str(0))),
        "replace" => Value::str(s.borrow().replace(&arg_str(0), &arg_str(1))),
        "repeat" => {
            let n = match args.first() {
                Some(Value::Int(n)) => *n as usize,
                _ => 0,
            };
            Value::str(s.borrow().repeat(n))
        }
        "chars" => Value::vec(s.borrow().chars().map(Value::Char).collect()),
        "lines" => Value::vec(s.borrow().lines().map(Value::str).collect()),
        "split" => {
            let sep = arg_str(0);
            Value::vec(s.borrow().split(&sep).map(Value::str).collect())
        }
        "split_whitespace" => {
            Value::vec(s.borrow().split_whitespace().map(Value::str).collect())
        }
        "count" => Value::Int(s.borrow().chars().count() as i64),
        "as_str" | "as_string" => Value::some(Value::str(s.borrow().clone())),
        "cmp" => make_ordering(s.borrow().as_str().cmp(arg_str(0).as_str())),
        "parse" => {
            let text = s.borrow();
            let t = text.trim();
            if let Ok(i) = t.parse::<i64>() {
                Value::ok(Value::Int(i))
            } else if let Ok(f) = t.parse::<f64>() {
                Value::ok(Value::Float(f))
            } else if let Ok(b) = t.parse::<bool>() {
                Value::ok(Value::Bool(b))
            } else {
                Value::err(Value::str(format!("cannot parse `{t}`")))
            }
        }
        _ => {
            if let Some(colored) = color_method(&s.borrow(), name) {
                colored
            } else {
                bail!("unknown method `{name}` on String")
            }
        }
    })
}

/// The `colored` crate as string methods. Returns the styled text as a plain
/// string carrying ANSI codes, so chaining and printing both work. Honors the
/// crate's own NO_COLOR and terminal detection.
fn color_method(s: &str, name: &str) -> Option<Value> {
    use colored::Colorize;
    let out = match name {
        "red" => s.red(),
        "green" => s.green(),
        "yellow" => s.yellow(),
        "blue" => s.blue(),
        "magenta" | "purple" => s.magenta(),
        "cyan" => s.cyan(),
        "white" => s.white(),
        "black" => s.black(),
        "bright_red" => s.bright_red(),
        "bright_green" => s.bright_green(),
        "bright_yellow" => s.bright_yellow(),
        "bright_blue" => s.bright_blue(),
        "bright_cyan" => s.bright_cyan(),
        "on_red" => s.on_red(),
        "on_green" => s.on_green(),
        "on_blue" => s.on_blue(),
        "bold" => s.bold(),
        "dimmed" => s.dimmed(),
        "italic" => s.italic(),
        "underline" => s.underline(),
        "reversed" => s.reversed(),
        "clear" | "normal" => s.normal(),
        _ => return None,
    };
    Some(Value::str(out.to_string()))
}

fn vec_method(v: &Rc<RefCell<Vec<Value>>>, name: &str, args: &[Value]) -> Result<Value> {
    Ok(match name {
        "len" => Value::Int(v.borrow().len() as i64),
        "is_empty" => Value::Bool(v.borrow().is_empty()),
        "clone" => Value::vec(v.borrow().clone()),
        "push" => {
            v.borrow_mut().push(args.first().cloned().unwrap_or(Value::Unit));
            Value::Unit
        }
        "pop" => match v.borrow_mut().pop() {
            Some(x) => Value::some(x),
            None => Value::none(),
        },
        "insert" => {
            let i = int_arg(args, 0)? as usize;
            v.borrow_mut().insert(i, args.get(1).cloned().unwrap_or(Value::Unit));
            Value::Unit
        }
        "remove" => {
            let i = int_arg(args, 0)? as usize;
            v.borrow_mut().remove(i)
        }
        "get" => {
            let i = int_arg(args, 0)? as usize;
            match v.borrow().get(i) {
                Some(x) => Value::some(x.clone()),
                None => Value::none(),
            }
        }
        "first" => v.borrow().first().cloned().map(Value::some).unwrap_or_else(Value::none),
        "last" => v.borrow().last().cloned().map(Value::some).unwrap_or_else(Value::none),
        "contains" => {
            let needle = args.first().cloned().unwrap_or(Value::Unit);
            Value::Bool(v.borrow().iter().any(|x| x.eq_value(&needle)))
        }
        "reverse" => {
            v.borrow_mut().reverse();
            Value::Unit
        }
        "sort" => {
            let mut items = v.borrow_mut();
            items.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
            Value::Unit
        }
        "dedup" => {
            let mut items = v.borrow_mut();
            items.dedup_by(|a, b| a.eq_value(b));
            Value::Unit
        }
        "clear" => {
            v.borrow_mut().clear();
            Value::Unit
        }
        "extend" | "append" => {
            if let Some(Value::Vec(other)) = args.first() {
                v.borrow_mut().extend(other.borrow().iter().cloned());
            }
            Value::Unit
        }
        "join" => {
            let sep = args.first().map(|v| v.display()).unwrap_or_default();
            let joined = v
                .borrow()
                .iter()
                .map(|x| x.display())
                .collect::<Vec<_>>()
                .join(&sep);
            Value::str(joined)
        }
        "sum" => {
            let mut acc_i = 0i64;
            let mut acc_f = 0f64;
            let mut is_float = false;
            for x in v.borrow().iter() {
                match x {
                    Value::Int(i) => acc_i += i,
                    Value::Float(f) => {
                        is_float = true;
                        acc_f += f;
                    }
                    _ => bail!("sum needs numbers"),
                }
            }
            if is_float {
                Value::Float(acc_f + acc_i as f64)
            } else {
                Value::Int(acc_i)
            }
        }
        "iter" | "into_iter" | "to_vec" | "collect" | "cloned" => Value::vec(v.borrow().clone()),
        "count" => Value::Int(v.borrow().len() as i64),
        "rev" => {
            let mut items = v.borrow().clone();
            items.reverse();
            Value::vec(items)
        }
        "enumerate" => Value::vec(
            v.borrow()
                .iter()
                .enumerate()
                .map(|(i, x)| {
                    Value::Tuple(Rc::new(RefCell::new(vec![Value::Int(i as i64), x.clone()])))
                })
                .collect(),
        ),
        "take" => {
            let n = int_arg(args, 0)? as usize;
            Value::vec(v.borrow().iter().take(n).cloned().collect())
        }
        "skip" => {
            let n = int_arg(args, 0)? as usize;
            Value::vec(v.borrow().iter().skip(n).cloned().collect())
        }
        _ => bail!("unknown method `{name}` on Vec"),
    })
}

fn map_method(m: &Rc<RefCell<BTreeMap<MapKey, Value>>>, name: &str, args: &[Value]) -> Result<Value> {
    let key = |i: usize| -> Result<MapKey> {
        args.get(i)
            .and_then(|v| v.as_key())
            .ok_or_else(|| anyhow!("invalid map key"))
    };
    Ok(match name {
        "len" => Value::Int(m.borrow().len() as i64),
        "is_empty" => Value::Bool(m.borrow().is_empty()),
        "clone" => Value::Map(Rc::new(RefCell::new(m.borrow().clone()))),
        "insert" => {
            let k = key(0)?;
            let old = m
                .borrow_mut()
                .insert(k, args.get(1).cloned().unwrap_or(Value::Unit));
            match old {
                Some(v) => Value::some(v),
                None => Value::none(),
            }
        }
        "get" => match m.borrow().get(&key(0)?) {
            Some(v) => Value::some(v.clone()),
            None => Value::none(),
        },
        "contains_key" => Value::Bool(m.borrow().contains_key(&key(0)?)),
        "remove" => match m.borrow_mut().remove(&key(0)?) {
            Some(v) => Value::some(v),
            None => Value::none(),
        },
        "keys" => Value::vec(m.borrow().keys().map(|k| k.to_value()).collect()),
        "values" | "values_mut" => Value::vec(m.borrow().values().cloned().collect()),
        "entry" => {
            let mut f = Fields::new();
            f.insert("map".into(), Value::Map(m.clone()));
            f.insert("key".into(), args.first().cloned().unwrap_or(Value::Unit));
            Value::Struct {
                name: "Entry".into(),
                fields: Rc::new(RefCell::new(f)),
            }
        }
        "iter" | "into_iter" | "drain" => Value::vec(
            m.borrow()
                .iter()
                .map(|(k, v)| {
                    Value::Tuple(Rc::new(RefCell::new(vec![k.to_value(), v.clone()])))
                })
                .collect(),
        ),
        _ => bail!("unknown method `{name}` on HashMap"),
    })
}

fn num_method(recv: &Value, name: &str, args: &[Value]) -> Result<Value> {
    let as_f = || match recv {
        Value::Int(i) => *i as f64,
        Value::Float(f) => *f,
        _ => 0.0,
    };
    Ok(match (recv, name) {
        (_, "to_string") => Value::str(recv.display()),
        (_, "clone") => recv.clone(),
        (Value::Int(i), "as_i64" | "as_u64" | "as_i128" | "as_usize") => Value::some(Value::Int(*i)),
        (_, "as_f64") => Value::some(Value::Float(as_f())),
        (Value::Int(i), "abs") => Value::Int(i.abs()),
        (Value::Float(f), "abs") => Value::Float(f.abs()),
        (Value::Int(i), "pow") => Value::Int(i.pow(int_arg(args, 0)? as u32)),
        (Value::Float(f), "powi") => Value::Float(f.powi(int_arg(args, 0)? as i32)),
        (Value::Float(f), "powf") => Value::Float(f.powf(float_arg(args, 0)?)),
        (Value::Float(_), "sqrt") => Value::Float(as_f().sqrt()),
        (Value::Float(f), "floor") => Value::Float(f.floor()),
        (Value::Float(f), "ceil") => Value::Float(f.ceil()),
        (Value::Float(f), "round") => Value::Float(f.round()),
        (Value::Int(a), "min") => Value::Int((*a).min(int_arg(args, 0)?)),
        (Value::Int(a), "max") => Value::Int((*a).max(int_arg(args, 0)?)),
        (Value::Int(a), "cmp") => make_ordering(a.cmp(&int_arg(args, 0)?)),
        (_, "partial_cmp") => Value::some(make_ordering(
            as_f()
                .partial_cmp(&float_arg(args, 0)?)
                .unwrap_or(std::cmp::Ordering::Equal),
        )),
        _ => bail!("unknown numeric method `{name}`"),
    })
}

fn opt_method(recv: &Value, name: &str, args: &[Value]) -> Result<Value> {
    let (variant, inner) = match recv {
        Value::Enum { variant, data, .. } => (variant.clone(), data.borrow().first().cloned()),
        _ => unreachable!(),
    };
    let is_some = variant == "Some";
    Ok(match name {
        "is_some" => Value::Bool(is_some),
        "is_none" => Value::Bool(!is_some),
        "clone" => recv.clone(),
        "unwrap" => inner.ok_or_else(|| anyhow!("called unwrap on a None value"))?,
        "expect" => inner
            .ok_or_else(|| anyhow!("{}", args.first().map(|v| v.display()).unwrap_or_default()))?,
        "unwrap_or" => inner.unwrap_or_else(|| args.first().cloned().unwrap_or(Value::Unit)),
        "unwrap_or_default" => inner.unwrap_or(Value::Unit),
        "cloned" | "copied" | "as_ref" | "as_deref" | "take" | "as_mut" => recv.clone(),
        "ok_or" => match inner {
            Some(v) => Value::ok(v),
            None => Value::err(args.first().cloned().unwrap_or(Value::Unit)),
        },
        _ => bail!("unknown method `{name}` on Option"),
    })
}

fn res_method(recv: &Value, name: &str, args: &[Value]) -> Result<Value> {
    let (variant, inner) = match recv {
        Value::Enum { variant, data, .. } => (variant.clone(), data.borrow().first().cloned()),
        _ => unreachable!(),
    };
    let is_ok = variant == "Ok";
    Ok(match name {
        "is_ok" => Value::Bool(is_ok),
        "is_err" => Value::Bool(!is_ok),
        "clone" => recv.clone(),
        "unwrap" => {
            if is_ok {
                inner.unwrap_or(Value::Unit)
            } else {
                bail!("called unwrap on an Err value: {}", inner.map(|v| v.display()).unwrap_or_default());
            }
        }
        "expect" => {
            if is_ok {
                inner.unwrap_or(Value::Unit)
            } else {
                bail!("{}", args.first().map(|v| v.display()).unwrap_or_default());
            }
        }
        "unwrap_or" => {
            if is_ok {
                inner.unwrap_or(Value::Unit)
            } else {
                args.first().cloned().unwrap_or(Value::Unit)
            }
        }
        "ok" => {
            if is_ok {
                Value::some(inner.unwrap_or(Value::Unit))
            } else {
                Value::none()
            }
        }
        "err" => {
            if is_ok {
                Value::none()
            } else {
                Value::some(inner.unwrap_or(Value::Unit))
            }
        }
        "context" | "with_context" => {
            if is_ok {
                Value::ok(inner.unwrap_or(Value::Unit))
            } else {
                let ctx = args.first().map(|v| v.display()).unwrap_or_default();
                let cause = inner.map(|v| v.display()).unwrap_or_default();
                Value::err(Value::str(format!("{ctx}\nCaused by: {cause}")))
            }
        }
        _ => bail!("unknown method `{name}` on Result"),
    })
}

fn int_arg(args: &[Value], i: usize) -> Result<i64> {
    match args.get(i) {
        Some(Value::Int(n)) => Ok(*n),
        _ => bail!("expected an integer argument"),
    }
}

fn float_arg(args: &[Value], i: usize) -> Result<f64> {
    match args.get(i) {
        Some(Value::Float(f)) => Ok(*f),
        Some(Value::Int(n)) => Ok(*n as f64),
        _ => bail!("expected a float argument"),
    }
}

/// Ordering key for `sort`, good enough for numbers and strings.
fn sort_key(v: &Value) -> SortKey {
    match v {
        Value::Int(i) => SortKey::Int(*i),
        Value::Float(f) => SortKey::Float(*f),
        Value::Bool(b) => SortKey::Int(*b as i64),
        Value::Str(s) => SortKey::Str(s.borrow().clone()),
        Value::Char(c) => SortKey::Str(c.to_string()),
        Value::Tuple(items) | Value::Vec(items) => {
            SortKey::List(items.borrow().iter().map(sort_key).collect())
        }
        other => SortKey::Str(other.display()),
    }
}

#[derive(PartialEq)]
enum SortKey {
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<SortKey>),
}

impl Eq for SortKey {}

impl PartialOrd for SortKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SortKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (self, other) {
            (SortKey::Int(a), SortKey::Int(b)) => a.cmp(b),
            (SortKey::Float(a), SortKey::Float(b)) => {
                a.partial_cmp(b).unwrap_or(Ordering::Equal)
            }
            (SortKey::Int(a), SortKey::Float(b)) => {
                (*a as f64).partial_cmp(b).unwrap_or(Ordering::Equal)
            }
            (SortKey::Float(a), SortKey::Int(b)) => {
                a.partial_cmp(&(*b as f64)).unwrap_or(Ordering::Equal)
            }
            (SortKey::Str(a), SortKey::Str(b)) => a.cmp(b),
            (SortKey::List(a), SortKey::List(b)) => a.cmp(b),
            (SortKey::Int(_) | SortKey::Float(_), _) => Ordering::Less,
            (_, SortKey::Int(_) | SortKey::Float(_)) => Ordering::Greater,
            (SortKey::Str(_), SortKey::List(_)) => Ordering::Less,
            (SortKey::List(_), SortKey::Str(_)) => Ordering::Greater,
        }
    }
}
