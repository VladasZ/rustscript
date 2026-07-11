//! The `Send + Sync` value model for the parallel engine, used by `#[tokio::main]`
//! scripts. It mirrors `value.rs` but swaps `Rc` for `Arc` and `RefCell` for a
//! `parking_lot::Mutex`, so a value can move between worker threads and be
//! shared by concurrent tasks. The fast engine keeps its `Rc` model untouched,
//! so nothing here can slow the single threaded path.

use std::fmt::Write as _;
use std::sync::Arc;

use indexmap::IndexMap;
use parking_lot::Mutex;

use super::bytecode::Const;
use super::pnative::PNative;

/// Shared, growable list. `Arc` for cross thread sharing, `Mutex` for the
/// interior mutation the interpreter needs since it ignores ownership.
pub type PList = Arc<Mutex<Vec<PValue>>>;
pub type PMap = Arc<Mutex<IndexMap<PKey, PValue>>>;

/// Field layout of a struct, shared by every instance from the same site.
pub struct PStructShape {
    pub name: Arc<str>,
    pub fields: Vec<Arc<str>>,
    pub renames: Vec<Option<Arc<str>>>,
}

impl PStructShape {
    pub fn new(name: impl Into<Arc<str>>, fields: Vec<Arc<str>>) -> Arc<PStructShape> {
        Arc::new(PStructShape { name: name.into(), fields, renames: Vec::new() })
    }

    pub fn slot(&self, field: &str) -> Option<usize> {
        self.fields.iter().position(|f| &**f == field)
    }
}

/// A struct instance: its shape plus one value per field, in shape order.
pub struct PStructData {
    pub shape: Arc<PStructShape>,
    pub values: Mutex<Vec<PValue>>,
}

impl PStructData {
    pub fn name(&self) -> &Arc<str> {
        &self.shape.name
    }

    pub fn get(&self, field: &str) -> Option<PValue> {
        self.shape.slot(field).map(|i| self.values.lock()[i].clone())
    }

    pub fn set(&self, field: &str, v: PValue) -> bool {
        match self.shape.slot(field) {
            Some(i) => {
                self.values.lock()[i] = v;
                true
            }
            None => false,
        }
    }
}

/// A compiled closure body plus its captured upvalues.
pub struct PClosure {
    pub chunk: Arc<super::pchunk::PChunk>,
    pub captured: Vec<PValue>,
}

/// A runtime value that is `Send + Sync`.
#[derive(Clone, Default)]
pub enum PValue {
    #[default]
    Unit,
    Bool(bool),
    Int(i64),
    Float(f64),
    Char(char),
    Str(Arc<str>),
    Vec(PList),
    Map(PMap),
    Tuple(PList),
    Struct(Arc<PStructData>),
    Enum {
        enum_name: Arc<str>,
        variant: Arc<str>,
        data: Arc<[PValue]>,
    },
    Range {
        start: i64,
        end: i64,
        inclusive: bool,
    },
    Closure(Arc<PClosure>),
    Native(Arc<Mutex<PNative>>),
}

/// Hashable map key, the subset of values that may be keys.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum PKey {
    Bool(bool),
    Int(i64),
    Char(char),
    Str(Arc<str>),
}

impl PValue {
    pub fn str(s: impl Into<Arc<str>>) -> PValue {
        PValue::Str(s.into())
    }

    pub fn vec(items: Vec<PValue>) -> PValue {
        PValue::Vec(Arc::new(Mutex::new(items)))
    }

    pub fn tuple(items: Vec<PValue>) -> PValue {
        PValue::Tuple(Arc::new(Mutex::new(items)))
    }

    pub fn map() -> PValue {
        PValue::Map(Arc::new(Mutex::new(IndexMap::default())))
    }

    pub fn structure(shape: Arc<PStructShape>, values: Vec<PValue>) -> PValue {
        PValue::Struct(Arc::new(PStructData { shape, values: Mutex::new(values) }))
    }

    pub fn struct_of(
        name: impl Into<Arc<str>>,
        pairs: impl IntoIterator<Item = (Arc<str>, PValue)>,
    ) -> PValue {
        let (fields, values): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
        PValue::structure(PStructShape::new(name, fields), values)
    }

    pub fn some(v: PValue) -> PValue {
        PValue::Enum {
            enum_name: Arc::from("Option"),
            variant: Arc::from("Some"),
            data: Arc::from(vec![v]),
        }
    }

    pub fn none() -> PValue {
        PValue::Enum {
            enum_name: Arc::from("Option"),
            variant: Arc::from("None"),
            data: Arc::from(Vec::new()),
        }
    }

    pub fn ok(v: PValue) -> PValue {
        PValue::Enum {
            enum_name: Arc::from("Result"),
            variant: Arc::from("Ok"),
            data: Arc::from(vec![v]),
        }
    }

    pub fn err(v: PValue) -> PValue {
        PValue::Enum {
            enum_name: Arc::from("Result"),
            variant: Arc::from("Err"),
            data: Arc::from(vec![v]),
        }
    }

    pub fn is_truthy(&self) -> bool {
        matches!(self, PValue::Bool(true))
    }

    pub fn from_const(c: &Const) -> PValue {
        match c {
            Const::Float(f) => PValue::Float(*f),
            Const::Char(ch) => PValue::Char(*ch),
            Const::Str(s) => PValue::str(&**s),
            Const::Bytes(bytes) => {
                PValue::vec(bytes.iter().map(|&b| PValue::Int(b as i64)).collect())
            }
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            PValue::Unit => "()",
            PValue::Bool(_) => "bool",
            PValue::Int(_) => "integer",
            PValue::Float(_) => "float",
            PValue::Char(_) => "char",
            PValue::Str(_) => "String",
            PValue::Vec(_) => "Vec",
            PValue::Map(_) => "HashMap",
            PValue::Tuple(_) => "tuple",
            PValue::Struct(_) => "struct",
            PValue::Enum { .. } => "enum",
            PValue::Range { .. } => "range",
            PValue::Closure(_) => "closure",
            PValue::Native(_) => "native",
        }
    }

    pub fn as_key(&self) -> Option<PKey> {
        Some(match self {
            PValue::Bool(b) => PKey::Bool(*b),
            PValue::Int(i) => PKey::Int(*i),
            PValue::Char(c) => PKey::Char(*c),
            PValue::Str(s) => PKey::Str(s.clone()),
            _ => return None,
        })
    }

    pub fn eq_value(&self, other: &PValue) -> bool {
        match (self, other) {
            (PValue::Unit, PValue::Unit) => true,
            (PValue::Bool(a), PValue::Bool(b)) => a == b,
            (PValue::Int(a), PValue::Int(b)) => a == b,
            (PValue::Float(a), PValue::Float(b)) => a == b,
            (PValue::Int(a), PValue::Float(b)) | (PValue::Float(b), PValue::Int(a)) => {
                *a as f64 == *b
            }
            (PValue::Char(a), PValue::Char(b)) => a == b,
            (PValue::Str(a), PValue::Str(b)) => a == b,
            (PValue::Vec(a), PValue::Vec(b)) | (PValue::Tuple(a), PValue::Tuple(b)) => {
                let (a, b) = (a.lock(), b.lock());
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.eq_value(y))
            }
            (
                PValue::Enum { enum_name: ea, variant: va, data: da },
                PValue::Enum { enum_name: eb, variant: vb, data: db },
            ) => {
                ea == eb
                    && va == vb
                    && da.len() == db.len()
                    && da.iter().zip(db.iter()).all(|(x, y)| x.eq_value(y))
            }
            (PValue::Struct(a), PValue::Struct(b)) => {
                a.name() == b.name() && {
                    let (va, vb) = (a.values.lock(), b.values.lock());
                    va.len() == vb.len()
                        && a.shape.fields.iter().zip(va.iter()).all(|(k, v)| {
                            b.get(k).map(|o| v.eq_value(&o)).unwrap_or(false)
                        })
                }
            }
            (PValue::Native(a), PValue::Native(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// `Display`, the `{}` format.
    pub fn display(&self) -> String {
        match self {
            PValue::Unit => "()".into(),
            PValue::Bool(b) => b.to_string(),
            PValue::Int(i) => i.to_string(),
            PValue::Float(f) => format_float(*f),
            PValue::Char(c) => c.to_string(),
            PValue::Str(s) => s.to_string(),
            other => other.debug(),
        }
    }

    /// `Debug`, the `{:?}` format.
    pub fn debug(&self) -> String {
        let mut out = String::new();
        self.write_debug(&mut out);
        out
    }

    fn write_debug(&self, out: &mut String) {
        match self {
            PValue::Unit => out.push_str("()"),
            PValue::Bool(b) => write!(out, "{b}").unwrap(),
            PValue::Int(i) => write!(out, "{i}").unwrap(),
            PValue::Float(f) => out.push_str(&format_float(*f)),
            PValue::Char(c) => write!(out, "{c:?}").unwrap(),
            PValue::Str(s) => write!(out, "{:?}", &**s).unwrap(),
            PValue::Range { start, end, inclusive } => {
                let sep = if *inclusive { "..=" } else { ".." };
                write!(out, "{start}{sep}{end}").unwrap();
            }
            PValue::Vec(items) => {
                out.push('[');
                for (i, v) in items.lock().iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    v.write_debug(out);
                }
                out.push(']');
            }
            PValue::Tuple(items) => {
                out.push('(');
                let items = items.lock();
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    v.write_debug(out);
                }
                if items.len() == 1 {
                    out.push(',');
                }
                out.push(')');
            }
            PValue::Map(map) => {
                out.push('{');
                for (i, (k, v)) in map.lock().iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    k.write_debug(out);
                    out.push_str(": ");
                    v.write_debug(out);
                }
                out.push('}');
            }
            PValue::Struct(s) => {
                write!(out, "{}", super::resolver::bare(s.name())).unwrap();
                let values = s.values.lock();
                if values.is_empty() {
                    return;
                }
                if s.shape.fields.iter().enumerate().all(|(i, f)| &**f == i.to_string()) {
                    out.push('(');
                    for (i, v) in values.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        v.write_debug(out);
                    }
                    out.push(')');
                    return;
                }
                out.push_str(" { ");
                for (i, (k, v)) in s.shape.fields.iter().zip(values.iter()).enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write!(out, "{k}: ").unwrap();
                    v.write_debug(out);
                }
                out.push_str(" }");
            }
            PValue::Closure(_) => out.push_str("<closure>"),
            PValue::Native(n) => write!(out, "<{}>", n.lock().type_name()).unwrap(),
            PValue::Enum { variant, data, .. } => {
                write!(out, "{variant}").unwrap();
                if !data.is_empty() {
                    out.push('(');
                    for (i, v) in data.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        v.write_debug(out);
                    }
                    out.push(')');
                }
            }
        }
    }
}

impl PKey {
    fn write_debug(&self, out: &mut String) {
        match self {
            PKey::Bool(b) => write!(out, "{b}").unwrap(),
            PKey::Int(i) => write!(out, "{i}").unwrap(),
            PKey::Char(c) => write!(out, "{c:?}").unwrap(),
            PKey::Str(s) => write!(out, "{:?}", &**s).unwrap(),
        }
    }

    pub fn to_value(&self) -> PValue {
        match self {
            PKey::Bool(b) => PValue::Bool(*b),
            PKey::Int(i) => PValue::Int(*i),
            PKey::Char(c) => PValue::Char(*c),
            PKey::Str(s) => PValue::Str(s.clone()),
        }
    }
}

fn format_float(f: f64) -> String {
    if f == f.trunc() && f.is_finite() {
        format!("{f:.0}")
    } else {
        f.to_string()
    }
}
