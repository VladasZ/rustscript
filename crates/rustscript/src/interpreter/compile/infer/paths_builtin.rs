//! The builtin owners of a path call. Numeric constructors, script type constructors, `std`
//! paths and the bridged crates.

use std::sync::Arc;

use syn::Expr;

use super::{Infer, Ty, type_arg};
use crate::interpreter::numeric::IntWidth;

impl Infer<'_, '_> {
    /// `i64::from(x)`, `u8::try_from(x)`, `f64::max(a, b)` and the other numeric paths.
    pub(super) fn numeric_call(&mut self, owner: &str, last: &str, args: &[&Expr]) -> Option<Ty> {
        Some(match (owner, last) {
            ("char", "from") => {
                self.walk_args(args);
                Ty::Char
            }
            ("char", "from_u32" | "from_digit") => {
                self.walk_args(args);
                Ty::option(Ty::Char)
            }
            ("f64", "from") => {
                self.walk_args(args);
                Ty::F64
            }
            ("f32", "from") => {
                self.walk_args(args);
                Ty::F32
            }
            (w, "from") if IntWidth::parse(w).is_some() => {
                self.walk_args(args);
                Ty::Int(IntWidth::parse(w).expect("checked"))
            }
            (w, "try_from") if IntWidth::parse(w).is_some() => {
                self.walk_args(args);
                Ty::result(
                    Ty::Int(IntWidth::parse(w).expect("checked")),
                    Ty::named("TryFromIntError"),
                )
            }
            (w, "from_str_radix") if IntWidth::parse(w).is_some() => {
                self.walk_args(args);
                Ty::result(
                    Ty::Int(IntWidth::parse(w).expect("checked")),
                    Ty::named("ParseIntError"),
                )
            }
            (
                w,
                "max" | "min" | "pow" | "abs" | "saturating_sub" | "saturating_add"
                | "wrapping_add" | "wrapping_sub",
            ) if IntWidth::parse(w).is_some() => {
                let ty = Ty::Int(IntWidth::parse(w).expect("checked"));
                for arg in args {
                    self.expr(arg, &ty);
                }
                ty
            }
            (
                "f64" | "f32",
                "max" | "min" | "sqrt" | "abs" | "powi" | "powf" | "floor" | "ceil" | "round",
            ) => {
                let ty = if owner == "f32" { Ty::F32 } else { Ty::F64 };
                for arg in args {
                    self.expr(arg, &ty);
                }
                ty
            }
            _ => return None,
        })
    }

    /// Constructors and the numeric conversions.
    pub(super) fn ctor_call(
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
    pub(super) fn std_call(
        &mut self,
        owner: &str,
        last: &str,
        args: &[&Expr],
        expected: &Ty,
    ) -> Option<Ty> {
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
    pub(super) fn external_call(
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

/// `i32::MAX`, `u8::MIN`, `f64::EPSILON`, `f64::consts::PI` and the like.
pub(super) fn numeric_constant(segs: &[String]) -> Option<Ty> {
    let (owner, name) = match segs {
        [.., owner, name] => (owner.as_str(), name.as_str()),
        _ => return None,
    };
    let owner = if owner == "consts" {
        match segs.len().checked_sub(3).and_then(|i| segs.get(i)) {
            Some(o) => o.as_str(),
            None => "f64",
        }
    } else {
        owner
    };
    match owner {
        "f64" => matches!(
            name,
            "MAX"
                | "MIN"
                | "EPSILON"
                | "INFINITY"
                | "NEG_INFINITY"
                | "NAN"
                | "PI"
                | "E"
                | "TAU"
                | "SQRT_2"
                | "LN_2"
                | "LN_10"
                | "FRAC_PI_2"
                | "MIN_POSITIVE"
        )
        .then_some(Ty::F64),
        "f32" => matches!(
            name,
            "MAX" | "MIN" | "EPSILON" | "INFINITY" | "NEG_INFINITY" | "NAN" | "PI" | "E" | "TAU"
        )
        .then_some(Ty::F32),
        "char" => matches!(name, "MAX" | "REPLACEMENT_CHARACTER").then_some(Ty::Char),
        other => {
            let width = IntWidth::parse(other)?;
            matches!(name, "MAX" | "MIN" | "BITS").then(|| {
                if name == "BITS" {
                    Ty::Int(IntWidth::U32)
                } else {
                    Ty::Int(width)
                }
            })
        }
    }
}
