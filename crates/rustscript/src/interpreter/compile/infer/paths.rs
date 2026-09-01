//! Path values and path calls, `None`, `Vec::new()`, `String::from(..)`, `fs::read_to_string`,
//! a script function, a constructor of a script type.

use std::collections::HashMap;
use std::sync::Arc;

use syn::Expr;

use super::generics::{bind_generic, erase, is_self_return, subst};
use super::numeric::numeric_constant;
use super::{Infer, Ty, type_arg};
use crate::interpreter::numeric::IntWidth;
use crate::interpreter::resolver::Res;

impl Infer<'_, '_> {
    pub(super) fn path_value(&mut self, p: &syn::ExprPath, expected: &Ty) -> Ty {
        if let Some(qself) = &p.qself {
            return self.lower(&qself.ty);
        }
        let segs: Vec<String> = p
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        if let [name] = segs.as_slice() {
            if let Some(ty) = self.lookup(name) {
                return ty;
            }
            if name == "None" {
                let payload = p
                    .path
                    .segments
                    .last()
                    .and_then(|seg| type_arg(seg, 0))
                    .map_or_else(|| expected.payload(), |t| self.lower(t));
                return Ty::option(payload);
            }
        }
        if let Some(ty) = numeric_constant(&segs) {
            return ty;
        }
        let segs = self.self_prefixed(segs);
        match self.ctx.resolver.resolve(self.ctx.module, &segs) {
            Ok(Res::Const(_)) => self
                .ctx
                .const_types
                .get(segs.join("::").as_str())
                .or_else(|| {
                    self.ctx
                        .const_types
                        .get(segs.last().map_or("", String::as_str))
                })
                .map_or(Ty::Unknown, |ty| self.lower(ty)),
            Ok(Res::Struct(canon)) => Ty::Struct(canon),
            Ok(Res::Enum(canon)) => Ty::Enum(canon),
            Ok(Res::TypeMember(canon, rest)) => {
                if self.ctx.resolver.enums.contains_key(&canon) && rest.len() == 1 {
                    return Ty::Enum(canon);
                }
                let key = format!(
                    "{}::{}",
                    crate::interpreter::resolver::bare(&canon),
                    rest.join("::")
                );
                if let Some(ty) = self.ctx.const_types.get(&key) {
                    return self.lower(ty);
                }
                // a method used as a value, `.map(Point::norm)`
                match self
                    .ctx
                    .impl_sigs
                    .get(&(canon.to_string(), rest.join("::")))
                {
                    Some(sig) => self.fn_value(&sig.clone(), Some(&self.user_type(&canon))),
                    None => Ty::Unknown,
                }
            }
            Ok(Res::Fn(_)) => match self
                .ctx
                .fn_signatures
                .get(segs.last().map_or("", String::as_str))
            {
                Some(sig) => self.fn_value(&sig.clone(), None),
                None => Ty::Unknown,
            },
            _ => Ty::Unknown,
        }
    }

    /// A function as a closure value.
    fn fn_value(&mut self, sig: &syn::Signature, self_ty: Option<&Ty>) -> Ty {
        let saved = self.swap_generics(sig);
        let params = sig
            .inputs
            .iter()
            .map(|input| match input {
                syn::FnArg::Receiver(_) => self_ty.cloned().unwrap_or(Ty::Unknown),
                syn::FnArg::Typed(t) => self.lower(&t.ty),
            })
            .collect();
        let ret = match &sig.output {
            syn::ReturnType::Type(_, ty) => self.lower(ty),
            syn::ReturnType::Default => Ty::Unit,
        };
        self.generics = saved;
        let ret = match (ret, self_ty) {
            (Ty::Unknown, Some(s)) if is_self_return(sig) => s.clone(),
            (ret, _) => ret,
        };
        Ty::Closure(params, Box::new(ret))
    }

    fn self_prefixed(&self, mut segs: Vec<String>) -> Vec<String> {
        if segs.first().is_some_and(|s| s == "Self")
            && let Some(ty) = self.ctx.impl_type
        {
            segs[0] = ty.to_string();
        }
        segs
    }

    /// `Some`, `Ok`, `Err`, `drop` and a local closure called by name.
    fn prelude_call(
        &mut self,
        name: &str,
        seg: &syn::PathSegment,
        args: &[&Expr],
        expected: &Ty,
    ) -> Option<Ty> {
        // `Ok::<T, E>(..)` states both sides, the expectation fills what it leaves out
        let stated = |i: usize| type_arg(seg, i).map(|t| self.lower(t));
        match name {
            "Some" => {
                let want = stated(0).unwrap_or_else(|| expected.payload());
                let payload = self.arg(args, 0, &want);
                return Some(Ty::option(payload));
            }
            "Ok" | "Err" => {
                let (ok, err) = match expected {
                    Ty::Result(ok, err) => ((**ok).clone(), (**err).clone()),
                    _ => (Ty::Unknown, Ty::Unknown),
                };
                let ok = stated(0).unwrap_or(ok);
                let err = stated(1).unwrap_or(err);
                if name == "Ok" {
                    let ok = self.arg(args, 0, &ok);
                    return Some(Ty::result(ok, err));
                }
                let err = self.arg(args, 0, &err);
                return Some(Ty::result(ok, err));
            }
            "drop" => {
                self.arg(args, 0, &Ty::Unknown);
                return Some(Ty::Unit);
            }
            _ => {}
        }
        if let Some(Ty::Closure(params, ret)) = self.lookup(name) {
            for (i, arg) in args.iter().enumerate() {
                let want = params.get(i).cloned().unwrap_or(Ty::Unknown);
                self.expr(arg, &want);
            }
            return Some(*ret);
        }
        None
    }

    pub(super) fn call(&mut self, c: &syn::ExprCall, expected: &Ty) -> Ty {
        let Expr::Path(path) = &*c.func else {
            // a closure value
            let callee = self.expr(&c.func, &Ty::Unknown);
            let (params, ret) = match callee {
                Ty::Closure(params, ret) => (params, *ret),
                _ => (Vec::new(), Ty::Unknown),
            };
            for (i, arg) in c.args.iter().enumerate() {
                let want = params.get(i).cloned().unwrap_or(Ty::Unknown);
                self.expr(arg, &want);
            }
            return ret;
        };
        if let Some(qself) = &path.qself {
            let owner = self.lower(&qself.ty);
            let name = path
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default();
            return self.assoc_call(&owner, &name, &path.path, c, expected);
        }
        let segs: Vec<String> = path
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        let args: Vec<&Expr> = c.args.iter().collect();
        if let [name] = segs.as_slice()
            && let Some(ty) = self.prelude_call(name, &path.path.segments[0], &args, expected)
        {
            return ty;
        }
        let segs = self.self_prefixed(segs);
        match self.ctx.resolver.resolve(self.ctx.module, &segs) {
            Ok(Res::Fn(_)) => {
                let name = segs.last().cloned().unwrap_or_default();
                match self.ctx.fn_signatures.get(&name).cloned() {
                    Some(sig) => self.sig_call(&sig, None, &args, expected),
                    None => self.walk_args(&args),
                }
            }
            Ok(Res::Struct(canon)) => {
                let fields = self.variant_fields(&path.path, &Ty::Struct(canon.clone()));
                for (i, arg) in args.iter().enumerate() {
                    let want = fields.get(i).map_or(Ty::Unknown, |(_, t)| t.clone());
                    self.expr(arg, &want);
                }
                Ty::Struct(canon)
            }
            Ok(Res::TypeMember(canon, rest)) => {
                let owner = self.user_type(&canon);
                if let Ty::Enum(_) = owner
                    && let [variant] = rest.as_slice()
                    && self
                        .ctx
                        .resolver
                        .enums
                        .get(&canon)
                        .is_some_and(|e| e.variants.iter().any(|v| v.ident == variant))
                {
                    let fields = self.variant_payload(&canon, variant);
                    for (i, arg) in args.iter().enumerate() {
                        let want = fields.get(i).map_or(Ty::Unknown, |(_, t)| t.clone());
                        self.expr(arg, &want);
                    }
                    return owner;
                }
                self.assoc_call(&owner, &rest.join("::"), &path.path, c, expected)
            }
            Ok(Res::Alias(m, target)) => {
                let owner = self.lower_in(&target, m);
                let name = segs.last().cloned().unwrap_or_default();
                self.assoc_call(&owner, &name, &path.path, c, expected)
            }
            _ => self.external_call(&segs, &path.path, &args, expected),
        }
    }

    /// `Type::name(..)` on a script type or a bridge type.
    fn assoc_call(
        &mut self,
        owner: &Ty,
        name: &str,
        path: &syn::Path,
        c: &syn::ExprCall,
        expected: &Ty,
    ) -> Ty {
        let args: Vec<&Expr> = c.args.iter().collect();
        let canon = match owner {
            Ty::Struct(c) | Ty::Enum(c) => Some(c.clone()),
            _ => None,
        };
        if let Some(canon) = canon
            && let Some(sig) = self
                .ctx
                .impl_sigs
                .get(&(canon.to_string(), name.to_string()))
                .cloned()
        {
            return self.sig_call(&sig, Some(owner), &args, expected);
        }
        if name == "default" && args.is_empty() {
            return owner.clone();
        }
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        self.external_call(&segs, path, &args, expected)
    }

    pub(super) fn walk_args(&mut self, args: &[&Expr]) -> Ty {
        for arg in args {
            self.expr(arg, &Ty::Unknown);
        }
        Ty::Unknown
    }

    fn arg(&mut self, args: &[&Expr], i: usize, want: &Ty) -> Ty {
        match args.get(i) {
            Some(arg) => self.expr(arg, want),
            None => Ty::Unknown,
        }
    }

    /// The generics of a signature shadow the enclosing ones while it is lowered.
    fn swap_generics(&mut self, sig: &syn::Signature) -> Vec<Arc<str>> {
        let own: Vec<Arc<str>> = sig
            .generics
            .type_params()
            .map(|p| Arc::from(p.ident.to_string().as_str()))
            .collect();
        std::mem::replace(&mut self.generics, own)
    }

    /// A call through a signature. Generic parameters take the type of the argument passed for
    /// them, so `fn pick<T>(a: T) -> T` returns what it was given.
    pub(super) fn sig_call(
        &mut self,
        sig: &syn::Signature,
        self_ty: Option<&Ty>,
        args: &[&Expr],
        expected: &Ty,
    ) -> Ty {
        let saved = self.swap_generics(sig);
        let params: Vec<Ty> = sig
            .inputs
            .iter()
            .filter_map(|input| match input {
                syn::FnArg::Receiver(_) => None,
                syn::FnArg::Typed(t) => Some(self.lower(&t.ty)),
            })
            .collect();
        let ret = match &sig.output {
            syn::ReturnType::Type(_, ty) => self.lower(ty),
            syn::ReturnType::Default => Ty::Unit,
        };
        self.generics = saved;
        let mut bound: HashMap<Arc<str>, Ty> = HashMap::new();
        // The expectation only hints a literal argument and fills a parameter no argument bound.
        // The arguments win, `E::from(pick(5usize, 6, false))` expects the payload of some `From`
        // impl and that must not decide what `pick` returns.
        let mut hint: HashMap<Arc<str>, Ty> = HashMap::new();
        bind_generic(&ret, expected, &mut hint);
        // the receiver of a method call sits before the typed params
        let skip = usize::from(sig.receiver().is_some());
        for (i, arg) in args.iter().enumerate() {
            let param = params
                .get(i.saturating_sub(skip))
                .cloned()
                .unwrap_or(Ty::Unknown);
            let param = if self_ty.is_some() && i < skip {
                Ty::Unknown
            } else {
                param
            };
            let want = subst(&subst(&param, &bound), &hint);
            let got = self.expr(arg, &erase(&want));
            bind_generic(&param, &got, &mut bound);
        }
        let ret = subst(&subst(&ret, &bound), &hint);
        match (&ret, self_ty) {
            (Ty::Unknown, Some(s)) if is_self_return(sig) => s.clone(),
            _ => erase(&ret),
        }
    }
    /// Constructors and the numeric conversions.
    fn ctor_call(
        &mut self,
        owner: &str,
        last: &str,
        args: &[&Expr],
        expected: &Ty,
        turbofish: (Option<Ty>, Option<Ty>),
        item_want: &Ty,
    ) -> Option<Ty> {
        let (turbofish, turbofish2) = turbofish;
        let item_want = item_want.clone();
        if let Some(ty) = self.numeric_call(owner, last, args) {
            return Some(ty);
        }
        Some(match (owner, last) {
            ("Vec" | "VecDeque", "new" | "with_capacity") => {
                self.walk_args(args);
                Ty::vec(item_want)
            }
            ("Vec" | "VecDeque", "from") => {
                let got = self.arg(args, 0, expected);
                match got {
                    Ty::Vec(_) => got,
                    other => Ty::vec(other.item()),
                }
            }
            ("HashSet" | "BTreeSet", "new" | "with_capacity") => {
                self.walk_args(args);
                Ty::Set(Box::new(item_want))
            }
            ("HashSet" | "BTreeSet", "from") => {
                let got = self.arg(args, 0, &Ty::Unknown);
                Ty::Set(Box::new(got.item()))
            }
            ("HashMap" | "BTreeMap" | "IndexMap", "new" | "with_capacity") => {
                self.walk_args(args);
                match (turbofish, turbofish2, expected) {
                    (Some(k), Some(v), _) => Ty::Map(Box::new(k), Box::new(v)),
                    (_, _, Ty::Map(..)) => expected.clone(),
                    _ => Ty::Map(Box::new(Ty::Unknown), Box::new(Ty::Unknown)),
                }
            }
            ("HashMap" | "BTreeMap", "from") => {
                let got = self.arg(args, 0, &Ty::Unknown);
                match got.item() {
                    Ty::Tuple(kv) if kv.len() == 2 => {
                        Ty::Map(Box::new(kv[0].clone()), Box::new(kv[1].clone()))
                    }
                    _ => Ty::Map(Box::new(Ty::Unknown), Box::new(Ty::Unknown)),
                }
            }
            ("String", "new" | "with_capacity" | "from" | "from_utf8_lossy") => {
                self.walk_args(args);
                Ty::Str
            }
            ("String", "from_utf8") => {
                self.walk_args(args);
                Ty::result(Ty::Str, Ty::named("FromUtf8Error"))
            }
            ("Box" | "Rc" | "Arc" | "RefCell" | "Cell" | "Mutex", "new")
            | ("Rc" | "Arc", "clone")
            | ("mem", "take") => self.arg(args, 0, expected),
            ("Rc" | "Arc", "strong_count") => {
                self.walk_args(args);
                Ty::usize()
            }
            ("Default", "default") => expected.clone(),
            ("Option", "default") => Ty::option(expected.payload()),
            (_, "default") if IntWidth::parse(owner).is_some() => {
                Ty::Int(IntWidth::parse(owner).expect("checked"))
            }
            ("f64", "default") => Ty::F64,
            ("f32", "default") => Ty::F32,
            ("bool", "default") => Ty::Bool,
            ("cmp" | "std::cmp", "min" | "max") => {
                let a = self.arg(args, 0, expected);
                let b = self.arg(args, 1, &a);
                self.vars.unify(&a, &b);
                a
            }
            ("mem", "replace") => {
                let got = self.arg(args, 0, expected);
                self.arg(args, 1, &got);
                got
            }
            ("mem", "swap") | ("thread", "sleep") => {
                self.walk_args(args);
                Ty::Unit
            }
            _ => return None,
        })
    }

    /// The `std` paths, `env`, `fs`, `io`, `Path`, `Command` and the clocks.
    fn std_call(&mut self, owner: &str, last: &str, args: &[&Expr], expected: &Ty) -> Option<Ty> {
        Some(match (owner, last) {
            ("env", "args") => Ty::iter(Ty::Str),
            ("env", "var") => {
                self.walk_args(args);
                Ty::result(Ty::Str, Ty::named("VarError"))
            }
            ("env", "vars") => Ty::iter(Ty::Tuple(vec![Ty::Str, Ty::Str])),
            ("env", "current_dir" | "home_dir" | "temp_dir") => {
                if last == "current_dir" {
                    Ty::result(Ty::named("PathBuf"), Ty::named("io::Error"))
                } else {
                    Ty::named("PathBuf")
                }
            }
            ("fs", "read_to_string") => {
                self.walk_args(args);
                Ty::result(Ty::Str, Ty::named("io::Error"))
            }
            ("fs", "read") => {
                self.walk_args(args);
                Ty::result(Ty::vec(Ty::Int(IntWidth::U8)), Ty::named("io::Error"))
            }
            ("fs", "read_dir") => {
                self.walk_args(args);
                Ty::result(
                    Ty::iter(Ty::result(Ty::named("DirEntry"), Ty::named("io::Error"))),
                    Ty::named("io::Error"),
                )
            }
            ("fs", "metadata" | "symlink_metadata") => {
                self.walk_args(args);
                Ty::result(Ty::named("Metadata"), Ty::named("io::Error"))
            }
            ("fs", "canonicalize" | "read_link") => {
                self.walk_args(args);
                Ty::result(Ty::named("PathBuf"), Ty::named("io::Error"))
            }
            ("fs", _) => {
                self.walk_args(args);
                Ty::result(Ty::Unit, Ty::named("io::Error"))
            }
            ("File", "open" | "create") | ("OpenOptions", "open") => {
                self.walk_args(args);
                Ty::result(Ty::named("File"), Ty::named("io::Error"))
            }
            ("Path", "new") => {
                self.walk_args(args);
                Ty::named("Path")
            }
            ("PathBuf", "from" | "new") => {
                self.walk_args(args);
                Ty::named("PathBuf")
            }
            ("Duration", _) => {
                self.walk_args(args);
                Ty::named("Duration")
            }
            ("Instant", "now") => Ty::named("Instant"),
            ("SystemTime", "now") => Ty::named("SystemTime"),
            ("Command", "new") => {
                self.walk_args(args);
                Ty::named("Command")
            }
            ("Regex", "new") => {
                self.walk_args(args);
                Ty::result(Ty::named("Regex"), Ty::named("regex::Error"))
            }
            ("io", "stdin") => Ty::named("Stdin"),
            ("io", "stdout") => Ty::named("Stdout"),
            ("io", "stderr") => Ty::named("Stderr"),
            ("iter", "repeat" | "once") => {
                let item = self.arg(args, 0, &expected.item());
                Ty::iter(item)
            }
            ("iter", "empty") => Ty::iter(expected.item()),
            _ => return None,
        })
    }

    /// Calls into the bridges, by the last segments of the path.
    fn external_call(
        &mut self,
        segs: &[String],
        path: &syn::Path,
        args: &[&Expr],
        expected: &Ty,
    ) -> Ty {
        let last = segs.last().map_or("", String::as_str);
        let owner = if segs.len() >= 2 {
            segs[segs.len() - 2].as_str()
        } else {
            ""
        };
        let owner_seg = path.segments.iter().rev().nth(1);
        let turbofish = owner_seg
            .and_then(|seg| type_arg(seg, 0))
            .map(|t| self.lower(t));
        let turbofish2 = owner_seg
            .and_then(|seg| type_arg(seg, 1))
            .map(|t| self.lower(t));
        let fn_turbofish = path
            .segments
            .last()
            .and_then(|seg| type_arg(seg, 0))
            .map(|t| self.lower(t));
        let item_want = turbofish.clone().unwrap_or_else(|| expected.item());
        if let Some(ty) = self.ctor_call(
            owner,
            last,
            args,
            expected,
            (turbofish, turbofish2),
            &item_want,
        ) {
            return ty;
        }
        if let Some(ty) = self.std_call(owner, last, args, expected) {
            return ty;
        }
        match (owner, last) {
            (
                "serde_json" | "serde_yaml" | "toml",
                "from_str" | "from_value" | "from_slice" | "from_reader",
            ) => {
                self.walk_args(args);
                let target = fn_turbofish.unwrap_or_else(|| expected.payload());
                Ty::result(target, Ty::named(&format!("{owner}::Error")))
            }
            ("serde_json" | "serde_yaml" | "toml", "to_string" | "to_string_pretty") => {
                self.walk_args(args);
                Ty::result(Ty::Str, Ty::named(&format!("{owner}::Error")))
            }
            ("serde_json", "to_value") => {
                self.walk_args(args);
                Ty::result(Ty::Json, Ty::named("serde_json::Error"))
            }
            ("tokio" | "task", "spawn") => self.arg(args, 0, &Ty::Unknown),
            ("time" | "tokio::time", "sleep") => {
                self.walk_args(args);
                Ty::Named(Arc::from("Future"), vec![Ty::Unit])
            }
            ("process", "exit") => {
                self.walk_args(args);
                Ty::Unknown
            }
            ("blocking" | "reqwest", "get") => {
                self.walk_args(args);
                Ty::result(Ty::named("Response"), Ty::named("reqwest::Error"))
            }
            ("HeaderMap", "new") => Ty::named("HeaderMap"),
            ("HeaderValue", "from_static") => {
                self.walk_args(args);
                Ty::named("HeaderValue")
            }
            ("HeaderValue", "from_str") => {
                self.walk_args(args);
                Ty::result(Ty::named("HeaderValue"), Ty::named("InvalidHeaderValue"))
            }
            ("Client", "new") => Ty::named("Client"),
            ("Client", "builder") => Ty::named("ClientBuilder"),
            ("Local" | "Utc", "now") => Ty::named("DateTime"),
            ("Uuid", "new_v4") => Ty::named("Uuid"),
            _ => self.walk_args(args),
        }
    }
}
