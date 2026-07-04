use std::cell::RefCell;
use std::fmt::Write;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use indexmap::{Equivalent, IndexMap};
use rustc_hash::FxBuildHasher;

use super::native::Native;

/// Struct fields keep their declaration order so serialization and debug output
/// match the real compiler. Keys are shared `Rc<str>` so building many
/// instances of one struct clones pointers, not strings.
pub type Fields = IndexMap<Rc<str>, Value, FxBuildHasher>;

/// Script HashMap storage. Hashed lookups, insertion ordered iteration.
/// Lookups by a borrowed key go through `KeyRef` so they never clone the key.
pub type Map = IndexMap<MapKey, Value, FxBuildHasher>;

/// A runtime value. Containers use `Rc<RefCell<..>>` so that `&mut` aliasing and
/// shared mutation behave, since the interpreter ignores ownership entirely.
#[derive(Clone)]
pub enum Value {
    Unit,
    Bool(bool),
    Int(i64),
    Float(f64),
    Char(char),
    Str(Rc<RefCell<String>>),
    Vec(Rc<RefCell<Vec<Value>>>),
    Map(Rc<RefCell<Map>>),
    Tuple(Rc<RefCell<Vec<Value>>>),
    /// Struct instance. Named fields, or positional for tuple structs.
    Struct {
        name: Rc<str>,
        fields: Rc<RefCell<Fields>>,
    },
    /// Enum value, including the builtin Option and Result. The payload is
    /// immutable once built, so it is a plain shared slice, not a RefCell.
    Enum {
        enum_name: Rc<str>,
        variant: Rc<str>,
        data: Rc<[Value]>,
    },
    Range {
        start: i64,
        end: i64,
        inclusive: bool,
    },
    Closure(Rc<ClosureData>),
    /// A live host resource: an open file, a child process, a socket, a
    /// buffered reader. Shared by `Rc` so the same handle can be passed around.
    Native(Rc<RefCell<Native>>),
}

/// A closure is a compiled body plus the upvalues it captured by value when it
/// was built. Container captures share their `Rc`, so mutation through them is
/// visible, matching a by-value capture of the handle.
pub struct ClosureData {
    pub chunk: Rc<super::bytecode::Chunk>,
    pub captured: Vec<Value>,
}

/// Hashable key for maps. Only a subset of values can be keys.
#[derive(Clone, PartialEq, Eq)]
pub enum MapKey {
    Bool(bool),
    Int(i64),
    Char(char),
    Str(String),
}

/// Hashes must not include the variant tag so a borrowed `KeyRef` lookup and a
/// stored `MapKey` land in the same bucket. Cross-variant collisions are fine,
/// equality still separates them.
impl Hash for MapKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            MapKey::Bool(b) => b.hash(state),
            MapKey::Int(i) => i.hash(state),
            MapKey::Char(c) => c.hash(state),
            MapKey::Str(s) => s.hash(state),
        }
    }
}

/// Borrowed view of a value used as a map key, so `get` and `contains_key` do
/// not clone the key string.
pub enum KeyRef<'a> {
    Bool(bool),
    Int(i64),
    Char(char),
    Str(&'a str),
}

impl Hash for KeyRef<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            KeyRef::Bool(b) => b.hash(state),
            KeyRef::Int(i) => i.hash(state),
            KeyRef::Char(c) => c.hash(state),
            KeyRef::Str(s) => s.hash(state),
        }
    }
}

impl Equivalent<MapKey> for KeyRef<'_> {
    fn equivalent(&self, key: &MapKey) -> bool {
        match (self, key) {
            (KeyRef::Bool(a), MapKey::Bool(b)) => a == b,
            (KeyRef::Int(a), MapKey::Int(b)) => a == b,
            (KeyRef::Char(a), MapKey::Char(b)) => a == b,
            (KeyRef::Str(a), MapKey::Str(b)) => *a == b.as_str(),
            _ => false,
        }
    }
}

thread_local! {
    /// Shared empty payload so `None` and unit variants allocate nothing.
    static EMPTY_DATA: Rc<[Value]> = Rc::from(Vec::new());
    static OPTION_NAME: Rc<str> = Rc::from("Option");
    static SOME_NAME: Rc<str> = Rc::from("Some");
    static NONE_NAME: Rc<str> = Rc::from("None");
    static RESULT_NAME: Rc<str> = Rc::from("Result");
    static OK_NAME: Rc<str> = Rc::from("Ok");
    static ERR_NAME: Rc<str> = Rc::from("Err");
}

pub fn fields_with_capacity(n: usize) -> Fields {
    Fields::with_capacity_and_hasher(n, FxBuildHasher)
}

pub fn map_with_capacity(n: usize) -> Map {
    Map::with_capacity_and_hasher(n, FxBuildHasher)
}

/// `Unit` default so registers can be moved out with `mem::take`.
impl Default for Value {
    fn default() -> Value {
        Value::Unit
    }
}

impl Value {
    pub fn str(s: impl Into<String>) -> Value {
        Value::Str(Rc::new(RefCell::new(s.into())))
    }

    pub fn vec(items: Vec<Value>) -> Value {
        Value::Vec(Rc::new(RefCell::new(items)))
    }

    pub fn empty_data() -> Rc<[Value]> {
        EMPTY_DATA.with(Rc::clone)
    }

    /// Single element enum payload, one allocation.
    pub fn one_data(v: Value) -> Rc<[Value]> {
        std::iter::once(v).collect()
    }

    pub fn some(v: Value) -> Value {
        Value::Enum {
            enum_name: OPTION_NAME.with(Rc::clone),
            variant: SOME_NAME.with(Rc::clone),
            data: Value::one_data(v),
        }
    }

    pub fn none() -> Value {
        Value::Enum {
            enum_name: OPTION_NAME.with(Rc::clone),
            variant: NONE_NAME.with(Rc::clone),
            data: Value::empty_data(),
        }
    }

    pub fn ok(v: Value) -> Value {
        Value::Enum {
            enum_name: RESULT_NAME.with(Rc::clone),
            variant: OK_NAME.with(Rc::clone),
            data: Value::one_data(v),
        }
    }

    pub fn err(v: Value) -> Value {
        Value::Enum {
            enum_name: RESULT_NAME.with(Rc::clone),
            variant: ERR_NAME.with(Rc::clone),
            data: Value::one_data(v),
        }
    }

    pub fn is_truthy(&self) -> bool {
        matches!(self, Value::Bool(true))
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Unit => "()",
            Value::Bool(_) => "bool",
            Value::Int(_) => "integer",
            Value::Float(_) => "float",
            Value::Char(_) => "char",
            Value::Str(_) => "String",
            Value::Vec(_) => "Vec",
            Value::Map(_) => "HashMap",
            Value::Tuple(_) => "tuple",
            Value::Struct { .. } => "struct",
            Value::Enum { .. } => "enum",
            Value::Range { .. } => "range",
            Value::Closure(_) => "closure",
            Value::Native(n) => n.borrow().type_name(),
        }
    }

    pub fn as_key(&self) -> Option<MapKey> {
        Some(match self {
            Value::Bool(b) => MapKey::Bool(*b),
            Value::Int(i) => MapKey::Int(*i),
            Value::Char(c) => MapKey::Char(*c),
            Value::Str(s) => MapKey::Str(s.borrow().clone()),
            _ => return None,
        })
    }

    /// Turn an owned value into a map key. A uniquely held string moves its
    /// buffer into the key instead of cloning it.
    pub fn into_key(self) -> Option<MapKey> {
        Some(match self {
            Value::Bool(b) => MapKey::Bool(b),
            Value::Int(i) => MapKey::Int(i),
            Value::Char(c) => MapKey::Char(c),
            Value::Str(s) => match Rc::try_unwrap(s) {
                Ok(cell) => MapKey::Str(cell.into_inner()),
                Err(rc) => MapKey::Str(rc.borrow().clone()),
            },
            _ => return None,
        })
    }

    /// Run `f` with a borrowed key view of this value, avoiding the string
    /// clone `as_key` pays. None when the value cannot be a key.
    pub fn with_key<R>(&self, f: impl FnOnce(Option<KeyRef>) -> R) -> R {
        match self {
            Value::Bool(b) => f(Some(KeyRef::Bool(*b))),
            Value::Int(i) => f(Some(KeyRef::Int(*i))),
            Value::Char(c) => f(Some(KeyRef::Char(*c))),
            Value::Str(s) => {
                let s = s.borrow();
                f(Some(KeyRef::Str(&s)))
            }
            _ => f(None),
        }
    }

    /// Value equality used by `==`, `match`, and map lookups.
    pub fn eq_value(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Unit, Value::Unit) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Int(a), Value::Float(b)) | (Value::Float(b), Value::Int(a)) => *a as f64 == *b,
            (Value::Char(a), Value::Char(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => *a.borrow() == *b.borrow(),
            (Value::Vec(a), Value::Vec(b)) | (Value::Tuple(a), Value::Tuple(b)) => {
                let (a, b) = (a.borrow(), b.borrow());
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.eq_value(y))
            }
            (
                Value::Enum {
                    enum_name: ea,
                    variant: va,
                    data: da,
                },
                Value::Enum {
                    enum_name: eb,
                    variant: vb,
                    data: db,
                },
            ) => {
                ea == eb
                    && va == vb
                    && da.len() == db.len()
                    && da.iter().zip(db.iter()).all(|(x, y)| x.eq_value(y))
            }
            (
                Value::Struct {
                    name: na,
                    fields: fa,
                },
                Value::Struct {
                    name: nb,
                    fields: fb,
                },
            ) => {
                na == nb && {
                    let (fa, fb) = (fa.borrow(), fb.borrow());
                    fa.len() == fb.len()
                        && fa
                            .iter()
                            .all(|(k, v)| fb.get(k).map(|o| v.eq_value(o)).unwrap_or(false))
                }
            }
            (Value::Native(a), Value::Native(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// `Display`, the `{}` format.
    pub fn display(&self) -> String {
        match self {
            Value::Unit => "()".into(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => format_float(*f),
            Value::Char(c) => c.to_string(),
            Value::Str(s) => s.borrow().clone(),
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
            Value::Unit => out.push_str("()"),
            Value::Bool(b) => write!(out, "{b}").unwrap(),
            Value::Int(i) => write!(out, "{i}").unwrap(),
            Value::Float(f) => out.push_str(&format_float(*f)),
            Value::Char(c) => write!(out, "{c:?}").unwrap(),
            Value::Str(s) => write!(out, "{:?}", s.borrow()).unwrap(),
            Value::Range {
                start,
                end,
                inclusive,
            } => {
                let sep = if *inclusive { "..=" } else { ".." };
                write!(out, "{start}{sep}{end}").unwrap();
            }
            Value::Vec(items) => {
                out.push('[');
                for (i, v) in items.borrow().iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    v.write_debug(out);
                }
                out.push(']');
            }
            Value::Tuple(items) => {
                out.push('(');
                let items = items.borrow();
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
            Value::Map(map) => {
                out.push('{');
                for (i, (k, v)) in map.borrow().iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    k.write_debug(out);
                    out.push_str(": ");
                    v.write_debug(out);
                }
                out.push('}');
            }
            Value::Struct { name, fields } => {
                write!(out, "{name}").unwrap();
                let fields = fields.borrow();
                if !fields.is_empty() {
                    out.push_str(" { ");
                    for (i, (k, v)) in fields.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        write!(out, "{k}: ").unwrap();
                        v.write_debug(out);
                    }
                    out.push_str(" }");
                }
            }
            Value::Closure(_) => out.push_str("<closure>"),
            Value::Native(n) => write!(out, "<{}>", n.borrow().type_name()).unwrap(),
            Value::Enum {
                variant, data, ..
            } => {
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

impl MapKey {
    fn write_debug(&self, out: &mut String) {
        match self {
            MapKey::Bool(b) => write!(out, "{b}").unwrap(),
            MapKey::Int(i) => write!(out, "{i}").unwrap(),
            MapKey::Char(c) => write!(out, "{c:?}").unwrap(),
            MapKey::Str(s) => write!(out, "{s:?}").unwrap(),
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            MapKey::Bool(b) => Value::Bool(*b),
            MapKey::Int(i) => Value::Int(*i),
            MapKey::Char(c) => Value::Char(*c),
            MapKey::Str(s) => Value::str(s.clone()),
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
