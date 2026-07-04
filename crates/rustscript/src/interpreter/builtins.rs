use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use anyhow::{Result, anyhow, bail};
use syn::{Expr, ExprCall};

use super::value::{MapKey, Value};
use super::{Flow, Frame, Interp, flow};

impl Interp {
    /// Resolve a path used as a value: a variable, `None`, or a unit enum variant.
    pub(super) fn eval_path(&self, path: &syn::Path, frame: &Frame) -> Result<Value> {
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        if segs.len() == 1 {
            let name = &segs[0];
            if let Some(v) = frame.get(name) {
                return Ok(v);
            }
            if name == "None" {
                return Ok(Value::none());
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

    pub(super) fn eval_call(&self, c: &ExprCall, frame: &mut Frame) -> Result<Flow> {
        let mut args = Vec::new();
        for a in &c.args {
            args.push(flow!(self.eval_expr(a, frame)));
        }
        let path = match &*c.func {
            Expr::Path(p) => &p.path,
            _ => bail!("cannot call this kind of expression"),
        };
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        let value = self.dispatch_call(&segs, args)?;
        // `serde_json::from_str::<T>(..)` and similar coerce the result into T.
        let value = match path.segments.last().and_then(super::eval::first_generic_type) {
            Some(ty) => self.coerce_result(value, ty),
            None => value,
        };
        Ok(Flow::Value(value))
    }

    fn dispatch_call(&self, segs: &[String], args: Vec<Value>) -> Result<Value> {
        let canon = self.canonical(segs);

        if canon.len() == 1 {
            let name = &canon[0];
            match name.as_str() {
                "Some" => return Ok(Value::some(one(args)?)),
                "Ok" => return Ok(Value::ok(one(args)?)),
                "Err" => return Ok(Value::err(one(args)?)),
                _ => {}
            }
            if let Some(f) = self.functions.get(name) {
                let f = f.clone();
                let mut frame = Frame::new();
                return self.call_fn_body(&f.block, &f.sig, &args, &mut frame);
            }
            if self.structs.contains_key(name) {
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
        if let Some(v) = native_call(ns, last, &args)? {
            return Ok(v);
        }
        // A method on a user type, `Type::assoc(..)` or UFCS `Type::method(recv, ..)`.
        if let Some(m) = self.methods.get(&(ns.clone(), last.clone())) {
            let m = m.clone();
            let mut frame = Frame::new();
            let has_receiver = matches!(m.sig.inputs.first(), Some(syn::FnArg::Receiver(_)));
            if has_receiver {
                let mut it = args.into_iter();
                let self_val = it
                    .next()
                    .ok_or_else(|| anyhow!("method `{ns}::{last}` needs a receiver"))?;
                frame.define("self", self_val);
                let rest: Vec<Value> = it.collect();
                return self.call_fn_body(&m.block, &m.sig, &rest, &mut frame);
            }
            return self.call_fn_body(&m.block, &m.sig, &args, &mut frame);
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
        let mut fields = BTreeMap::new();
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

    pub(super) fn eval_method(
        &self,
        recv: Value,
        name: &str,
        args: Vec<Value>,
        _frame: &mut Frame,
    ) -> Result<Value> {
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
            && let Some(m) = self.methods.get(&(tn.clone(), name.to_string()))
        {
            let m = m.clone();
            let mut frame = Frame::new();
            frame.define("self", recv);
            return self.call_fn_body(&m.block, &m.sig, &args, &mut frame);
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
                        found = Value::some(Value::Int(i as i128));
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
            Some(Value::Str(s)) => Ok(s.borrow().clone()),
            Some(other) => Ok(other.display()),
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
                Ok(n) => Value::ok(Value::Int(n as i128)),
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
            ("env", "args") => Value::vec(std::env::args().map(Value::str).collect()),
            ("env", "var") => match std::env::var(s(0)?) {
                Ok(v) => Value::ok(Value::str(v)),
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            ("env", "current_dir") => match std::env::current_dir() {
                Ok(p) => Value::ok(Value::str(p.display().to_string())),
                Err(e) => Value::err(Value::str(e.to_string())),
            },
            ("env", "set_var") => {
                // Safety: single threaded interpreter.
                unsafe { std::env::set_var(s(0)?, s(1)?) };
                Value::Unit
            }
            _ => return Ok(None),
        }))
    }

/// Run a `Command` value once it has been fully built, returning an `Output`.
fn run_command(fields: &Rc<RefCell<BTreeMap<String, Value>>>) -> Value {
    let f = fields.borrow();
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
    match cmd.output() {
        Ok(out) => {
            let mut o = BTreeMap::new();
            o.insert(
                "stdout".into(),
                Value::str(String::from_utf8_lossy(&out.stdout).into_owned()),
            );
            o.insert(
                "stderr".into(),
                Value::str(String::from_utf8_lossy(&out.stderr).into_owned()),
            );
            let mut st = BTreeMap::new();
            st.insert("code".into(), Value::Int(out.status.code().unwrap_or(-1) as i128));
            st.insert("success".into(), Value::Bool(out.status.success()));
            o.insert(
                "status".into(),
                Value::Struct {
                    name: "ExitStatus".into(),
                    fields: Rc::new(RefCell::new(st)),
                },
            );
            Value::ok(Value::Struct {
                name: "Output".into(),
                fields: Rc::new(RefCell::new(o)),
            })
        }
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

// -- ureq http bridge ------------------------------------------------------

/// Build an `HttpRequest` value for `ureq::get`, `ureq::post`, and friends.
fn make_request(func: &str, args: &[Value]) -> Option<Value> {
    let method = match func {
        "get" => "GET",
        "post" => "POST",
        "put" => "PUT",
        "delete" => "DELETE",
        "patch" => "PATCH",
        "head" => "HEAD",
        _ => return None,
    };
    let url = args.first().map(|v| v.display()).unwrap_or_default();
    let mut fields = BTreeMap::new();
    fields.insert("method".into(), Value::str(method));
    fields.insert("url".into(), Value::str(url));
    fields.insert("headers".into(), Value::vec(vec![]));
    Some(Value::Struct {
        name: "HttpRequest".into(),
        fields: Rc::new(RefCell::new(fields)),
    })
}

fn http_method(
    struct_name: &str,
    fields: &Rc<RefCell<BTreeMap<String, Value>>>,
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
    fields: &Rc<RefCell<BTreeMap<String, Value>>>,
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
        "query" | "timeout" => Ok(this()),
        _ => bail!("unknown method `{method}` on a request"),
    }
}

fn run_request(fields: &Rc<RefCell<BTreeMap<String, Value>>>, body: Option<String>) -> Value {
    let f = fields.borrow();
    let verb = f.get("method").map(|v| v.display()).unwrap_or_else(|| "GET".into());
    let url = f.get("url").map(|v| v.display()).unwrap_or_default();
    let mut headers = Vec::new();
    if let Some(Value::Vec(h)) = f.get("headers") {
        for item in h.borrow().iter() {
            if let Value::Tuple(pair) = item {
                let pair = pair.borrow();
                headers.push((pair[0].display(), pair[1].display()));
            }
        }
    }
    match do_http(&verb, &url, &headers, body) {
        Ok((status, text)) => {
            let mut rf = BTreeMap::new();
            rf.insert("status".into(), Value::Int(status as i128));
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
) -> Result<(u16, String)> {
    if matches!(method, "POST" | "PUT" | "PATCH") {
        let mut b = match method {
            "POST" => ureq::post(url),
            "PUT" => ureq::put(url),
            _ => ureq::patch(url),
        };
        for (k, v) in headers {
            b = b.header(k, v);
        }
        let mut resp = b.send(body.as_deref().unwrap_or(""))?;
        Ok((resp.status().as_u16(), resp.body_mut().read_to_string()?))
    } else {
        let mut b = match method {
            "DELETE" => ureq::delete(url),
            "HEAD" => ureq::head(url),
            _ => ureq::get(url),
        };
        for (k, v) in headers {
            b = b.header(k, v);
        }
        let mut resp = b.call()?;
        Ok((resp.status().as_u16(), resp.body_mut().read_to_string()?))
    }
}

fn response_method(fields: &Rc<RefCell<BTreeMap<String, Value>>>, method: &str) -> Value {
    let f = fields.borrow();
    match method {
        "status" => {
            let mut sf = BTreeMap::new();
            sf.insert("code".into(), f.get("status").cloned().unwrap_or(Value::Int(0)));
            Value::Struct {
                name: "StatusCode".into(),
                fields: Rc::new(RefCell::new(sf)),
            }
        }
        "body_mut" | "body" | "into_body" => {
            let mut bf = BTreeMap::new();
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

fn body_method(fields: &Rc<RefCell<BTreeMap<String, Value>>>, method: &str) -> Result<Value> {
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

fn status_method(fields: &Rc<RefCell<BTreeMap<String, Value>>>, method: &str) -> Value {
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

fn make_path(s: impl Into<String>) -> Value {
    let mut f = BTreeMap::new();
    f.insert("s".into(), Value::str(s.into()));
    Value::Struct {
        name: "Path".into(),
        fields: Rc::new(RefCell::new(f)),
    }
}

fn make_dir_entry(entry: &std::fs::DirEntry) -> Value {
    let mut f = BTreeMap::new();
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
    let mut f = BTreeMap::new();
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

fn path_string(fields: &Rc<RefCell<BTreeMap<String, Value>>>, key: &str) -> String {
    fields.borrow().get(key).map(|v| v.display()).unwrap_or_default()
}

fn path_method(
    fields: &Rc<RefCell<BTreeMap<String, Value>>>,
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
    fields: &Rc<RefCell<BTreeMap<String, Value>>>,
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

fn file_type_method(fields: &Rc<RefCell<BTreeMap<String, Value>>>, method: &str) -> Result<Value> {
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
    let mut f = BTreeMap::new();
    f.insert("pattern".into(), Value::str(pattern));
    Value::Struct {
        name: "Regex".into(),
        fields: Rc::new(RefCell::new(f)),
    }
}

fn make_match(m: &regex::Match) -> Value {
    let mut f = BTreeMap::new();
    f.insert("text".into(), Value::str(m.as_str().to_string()));
    f.insert("start".into(), Value::Int(m.start() as i128));
    f.insert("end".into(), Value::Int(m.end() as i128));
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
            names.insert(MapKey::Str(n.to_string()), Value::Int(i as i128));
        }
    }
    let mut f = BTreeMap::new();
    f.insert("groups".into(), Value::vec(groups));
    f.insert("names".into(), Value::Map(Rc::new(RefCell::new(names))));
    Value::Struct {
        name: "Captures".into(),
        fields: Rc::new(RefCell::new(f)),
    }
}

fn regex_method(
    fields: &Rc<RefCell<BTreeMap<String, Value>>>,
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

fn match_method(fields: &Rc<RefCell<BTreeMap<String, Value>>>, method: &str) -> Result<Value> {
    let f = fields.borrow();
    Ok(match method {
        "as_str" => f.get("text").cloned().unwrap_or_else(|| Value::str("")),
        "start" => f.get("start").cloned().unwrap_or(Value::Int(0)),
        "end" => f.get("end").cloned().unwrap_or(Value::Int(0)),
        _ => bail!("unknown method `{method}` on Match"),
    })
}

fn captures_method(
    fields: &Rc<RefCell<BTreeMap<String, Value>>>,
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
                Ok(Value::Int(g.borrow().len() as i128))
            } else {
                Ok(Value::Int(0))
            }
        }
        _ => bail!("unknown method `{method}` on Captures"),
    }
}

fn capture_group(fields: &Rc<RefCell<BTreeMap<String, Value>>>, i: usize) -> Value {
    match fields.borrow().get("groups") {
        Some(Value::Vec(g)) => g.borrow().get(i).cloned().unwrap_or_else(Value::none),
        _ => Value::none(),
    }
}

fn capture_name_index(fields: &Rc<RefCell<BTreeMap<String, Value>>>, name: &str) -> Option<usize> {
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
    fields: &Rc<RefCell<BTreeMap<String, Value>>>,
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
        Ok(bytes) => Value::ok(Value::vec(bytes.into_iter().map(|b| Value::Int(b as i128)).collect())),
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
            let mut fields = BTreeMap::new();
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
        _ => return Ok(None),
    }))
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
                Value::Int(i as i128)
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
    })
}

// -- builtin methods -------------------------------------------------------

fn builtin_method(recv: Value, name: &str, args: Vec<Value>) -> Result<Value> {
    match &recv {
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
    fields: &Rc<RefCell<BTreeMap<String, Value>>>,
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
        "env" => cmd_value(),
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
        "spawn" => bail!("unsupported: Command::spawn is not modeled, use output or status"),
        _ => bail!("unknown method `{name}` on Command"),
    })
}

fn exitstatus_method(fields: &Rc<RefCell<BTreeMap<String, Value>>>, name: &str) -> Result<Value> {
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
        "len" => Value::Int(s.borrow().len() as i128),
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
        "count" => Value::Int(s.borrow().chars().count() as i128),
        "as_str" | "as_string" => Value::some(Value::str(s.borrow().clone())),
        "cmp" => make_ordering(s.borrow().as_str().cmp(arg_str(0).as_str())),
        "parse" => {
            let text = s.borrow();
            let t = text.trim();
            if let Ok(i) = t.parse::<i128>() {
                Value::ok(Value::Int(i))
            } else if let Ok(f) = t.parse::<f64>() {
                Value::ok(Value::Float(f))
            } else if let Ok(b) = t.parse::<bool>() {
                Value::ok(Value::Bool(b))
            } else {
                Value::err(Value::str(format!("cannot parse `{t}`")))
            }
        }
        _ => bail!("unknown method `{name}` on String"),
    })
}

fn vec_method(v: &Rc<RefCell<Vec<Value>>>, name: &str, args: &[Value]) -> Result<Value> {
    Ok(match name {
        "len" => Value::Int(v.borrow().len() as i128),
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
            let mut acc_i = 0i128;
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
        "count" => Value::Int(v.borrow().len() as i128),
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
                    Value::Tuple(Rc::new(RefCell::new(vec![Value::Int(i as i128), x.clone()])))
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
        "len" => Value::Int(m.borrow().len() as i128),
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
        "values" => Value::vec(m.borrow().values().cloned().collect()),
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
        "cloned" | "copied" | "as_ref" | "as_deref" => recv.clone(),
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

fn int_arg(args: &[Value], i: usize) -> Result<i128> {
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
        Value::Str(s) => SortKey::Str(s.borrow().clone()),
        Value::Char(c) => SortKey::Str(c.to_string()),
        other => SortKey::Str(other.display()),
    }
}

#[derive(PartialEq)]
enum SortKey {
    Int(i128),
    Float(f64),
    Str(String),
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
            (SortKey::Int(_) | SortKey::Float(_), SortKey::Str(_)) => Ordering::Less,
            (SortKey::Str(_), _) => Ordering::Greater,
        }
    }
}
