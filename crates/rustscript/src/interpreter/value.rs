//! The `Send + Sync` value model, used by `#[tokio::main]`
//! scripts. It mirrors `value.rs` but swaps `Rc` for `Arc` and `RefCell` for a
//! `parking_lot::Mutex`, so a value can move between worker threads and be
//! shared by concurrent tasks.

use num_traits::AsPrimitive;
use std::fmt::Write as _;
use std::sync::Arc;

use indexmap::IndexMap;
use parking_lot::Mutex;

use super::bytecode::Const;
use super::native::Native;
use super::numeric::IntWidth;

/// Shared, growable list. `Arc` for cross thread sharing, `Mutex` for the
/// interior mutation the interpreter needs since it ignores ownership.
pub type List = Arc<Mutex<Vec<Value>>>;
pub type Map = Arc<Mutex<IndexMap<MapKey, Value>>>;

/// A set shares the map storage, each element stored as key -> Unit. The kind
/// is what makes iteration yield elements instead of pairs and routes the set
/// halves of `insert`, `remove`, and `contains`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MapKind {
    Map,
    Set,
}

pub enum ValueRef {
    VecElement { values: List, index: usize },
    MapEntry { map: Map, key: MapKey },
}

impl ValueRef {
    pub fn vec_element(values: List, index: usize) -> Self {
        Self::VecElement { values, index }
    }

    pub fn map_entry(map: Map, key: MapKey) -> Self {
        Self::MapEntry { map, key }
    }

    pub fn get(&self) -> Option<Value> {
        match self {
            Self::VecElement { values, index } => values.lock().get(*index).cloned(),
            Self::MapEntry { map, key } => map.lock().get(key).cloned(),
        }
    }

    pub fn set(&self, value: Value) -> bool {
        match self {
            Self::VecElement { values, index } => {
                let mut values = values.lock();
                let Some(slot) = values.get_mut(*index) else {
                    return false;
                };
                *slot = value;
                true
            }
            Self::MapEntry { map, key } => {
                map.lock().insert(key.clone(), value);
                true
            }
        }
    }
}

/// Field layout of a struct, shared by every instance from the same site.
/// The compiler emits these shapes directly, so runtime and bytecode share
/// one definition.
pub use super::bytecode::StructShape;

/// A struct instance: its shape plus one value per field, in shape order.
pub struct StructData {
    pub shape: Arc<StructShape>,
    pub values: Mutex<Vec<Value>>,
}

impl StructData {
    pub fn name(&self) -> &Arc<str> {
        &self.shape.name
    }

    pub fn get(&self, field: &str) -> Option<Value> {
        self.shape
            .slot(field)
            .map(|i| self.values.lock()[i].clone())
    }

    pub fn set(&self, field: &str, v: Value) -> bool {
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
#[derive(Clone)]
pub enum Upvalue {
    Value(Value),
    Mutable(Arc<Mutex<Value>>),
}

impl Upvalue {
    pub fn get(&self) -> Value {
        match self {
            Self::Value(value) => value.clone(),
            Self::Mutable(value) => value.lock().clone(),
        }
    }

    pub fn set(&self, value: Value) -> bool {
        let Self::Mutable(cell) = self else {
            return false;
        };
        *cell.lock() = value;
        true
    }
}

pub struct ClosureData {
    pub chunk: Arc<super::bytecode::Chunk>,
    pub captured: Vec<Upvalue>,
}

/// A runtime value that is `Send + Sync`.
#[derive(Clone, Default)]
pub enum Value {
    #[default]
    Unit,
    Bool(bool),
    Int(i64),
    /// An integer with a real width other than i64, in the storage form
    /// described in `numeric`.
    IntW(i64, IntWidth),
    Float(f64),
    /// A real f32, kept at f32 precision, mirroring `Value::F32`.
    F32(f32),
    Char(char),
    Str(Arc<str>),
    Vec(List),
    Map(Map, MapKind),
    Tuple(List),
    Struct(Arc<StructData>),
    Enum {
        enum_name: Arc<str>,
        variant: Arc<str>,
        data: Arc<[Value]>,
    },
    Range {
        start: i64,
        end: i64,
        inclusive: bool,
    },
    Closure(Arc<ClosureData>),
    Ref(Arc<ValueRef>),
    Native(Arc<Mutex<Native>>),
}

/// Hashable map key, the subset of values that may be keys.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum MapKey {
    Bool(bool),
    Int(i64),
    Char(char),
    Str(Arc<str>),
}

impl Value {
    pub fn str(s: impl Into<Arc<str>>) -> Value {
        Value::Str(s.into())
    }

    pub fn vec(items: Vec<Value>) -> Value {
        Value::Vec(Arc::new(Mutex::new(items)))
    }

    pub fn tuple(items: Vec<Value>) -> Value {
        Value::Tuple(Arc::new(Mutex::new(items)))
    }

    pub fn map() -> Value {
        Value::Map(Arc::new(Mutex::new(IndexMap::default())), MapKind::Map)
    }

    pub fn map_of(map: IndexMap<MapKey, Value>) -> Value {
        Value::Map(Arc::new(Mutex::new(map)), MapKind::Map)
    }

    pub fn set() -> Value {
        Value::Map(Arc::new(Mutex::new(IndexMap::default())), MapKind::Set)
    }

    pub fn set_of(map: IndexMap<MapKey, Value>) -> Value {
        Value::Map(Arc::new(Mutex::new(map)), MapKind::Set)
    }

    pub fn structure(shape: Arc<StructShape>, values: Vec<Value>) -> Value {
        Value::Struct(Arc::new(StructData {
            shape,
            values: Mutex::new(values),
        }))
    }

    pub fn struct_of(
        name: impl Into<Arc<str>>,
        pairs: impl IntoIterator<Item = (Arc<str>, Value)>,
    ) -> Value {
        let (fields, values): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
        Value::structure(StructShape::new(name, fields), values)
    }

    pub fn some(v: Value) -> Value {
        Value::Enum {
            enum_name: Arc::from("Option"),
            variant: Arc::from("Some"),
            data: Arc::from(vec![v]),
        }
    }

    pub fn none() -> Value {
        Value::Enum {
            enum_name: Arc::from("Option"),
            variant: Arc::from("None"),
            data: Arc::from(Vec::new()),
        }
    }

    /// True for `Option::None`, used to keep a null json value as None rather
    /// than wrapping it in Some when filling an Option struct field.
    pub fn is_none_value(&self) -> bool {
        matches!(self, Value::Enum { enum_name, variant, .. }
            if &**enum_name == "Option" && &**variant == "None")
    }

    pub fn ok(v: Value) -> Value {
        Value::Enum {
            enum_name: Arc::from("Result"),
            variant: Arc::from("Ok"),
            data: Arc::from(vec![v]),
        }
    }

    pub fn err(v: Value) -> Value {
        Value::Enum {
            enum_name: Arc::from("Result"),
            variant: Arc::from("Err"),
            data: Arc::from(vec![v]),
        }
    }

    pub fn is_truthy(&self) -> bool {
        matches!(self, Value::Bool(true))
    }

    pub fn from_const(c: &Const) -> Value {
        match c {
            Const::Float(f) => Value::Float(*f),
            Const::F32(f) => Value::F32(*f),
            Const::Char(ch) => Value::Char(*ch),
            Const::Str(s) => Value::str(&**s),
            Const::Bytes(bytes) => {
                Value::vec(bytes.iter().map(|&b| Value::Int(i64::from(b))).collect())
            }
        }
    }

    /// The value and width of an integer, tagged or plain. None otherwise.
    pub(super) fn int_parts(&self) -> Option<(i128, IntWidth)> {
        match self {
            Value::Int(i) => Some((i128::from(*i), IntWidth::I64)),
            Value::IntW(v, w) => Some((w.decode(*v), *w)),
            _ => None,
        }
    }

    /// Build an integer of the given width from an in-range value.
    pub(super) fn int_of_width(value: i128, width: IntWidth) -> Value {
        match width {
            IntWidth::I64 => Value::Int(i64::try_from(value).expect("truncated to width")),
            other => Value::IntW(other.encode(value), other),
        }
    }

    /// A tagged integer's value as an i64 when it fits.
    pub(super) fn untag_int(&self) -> Option<i64> {
        match self {
            Value::IntW(v, w) => i64::try_from(w.decode(*v)).ok(),
            _ => None,
        }
    }

    /// The i64 or f64 image of a width-tagged number, for the method and
    /// bridge surface that predates real widths. A u64 value past `i64::MAX`
    /// saturates, the clamp sentinels like `usize::MAX` always had here.
    /// None when the value is not tagged.
    pub(super) fn bridge_image(&self) -> Option<Value> {
        match self {
            Value::IntW(v, w) => {
                let value = w.decode(*v);
                Some(Value::Int(i64::try_from(value).unwrap_or(i64::MAX)))
            }
            Value::F32(f) => Some(Value::Float(f64::from(*f))),
            _ => None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Unit => "()",
            Value::Bool(_) => "bool",
            Value::Int(_) | Value::IntW(..) => "integer",
            Value::Float(_) | Value::F32(_) => "float",
            Value::Char(_) => "char",
            Value::Str(_) => "String",
            Value::Vec(_) => "Vec",
            Value::Map(_, MapKind::Map) => "HashMap",
            Value::Map(_, MapKind::Set) => "HashSet",
            Value::Tuple(_) => "tuple",
            Value::Struct(_) => "struct",
            Value::Enum { .. } => "enum",
            Value::Range { .. } => "range",
            Value::Closure(_) => "closure",
            Value::Ref(reference) => reference
                .get()
                .map_or("reference", |value| value.type_name()),
            Value::Native(_) => "native",
        }
    }

    pub fn as_key(&self) -> Option<MapKey> {
        Some(match self {
            Value::Bool(b) => MapKey::Bool(*b),
            Value::Int(i) => MapKey::Int(*i),
            // Unique per value within one width, and one real map never
            // mixes key widths.
            Value::IntW(v, _) => MapKey::Int(*v),
            Value::Char(c) => MapKey::Char(*c),
            Value::Str(s) => MapKey::Str(s.clone()),
            _ => return None,
        })
    }

    /// Turn an owned value into a map key. Strings hand over their buffer,
    /// no copy in any case.
    pub fn into_key(self) -> Option<MapKey> {
        Some(match self {
            Value::Bool(b) => MapKey::Bool(b),
            Value::Int(i) => MapKey::Int(i),
            Value::IntW(v, _) => MapKey::Int(v),
            Value::Char(c) => MapKey::Char(c),
            Value::Str(s) => MapKey::Str(s),
            _ => return None,
        })
    }

    pub fn eq_value(&self, other: &Value) -> bool {
        if let Value::Ref(reference) = self {
            return reference.get().is_some_and(|value| value.eq_value(other));
        }
        if let Value::Ref(reference) = other {
            return reference.get().is_some_and(|value| self.eq_value(&value));
        }
        match (self, other) {
            (Value::Unit, Value::Unit) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::IntW(..), Value::Int(_) | Value::IntW(..))
            | (Value::Int(_), Value::IntW(..)) => {
                self.int_parts().map(|(a, _)| a) == other.int_parts().map(|(b, _)| b)
            }
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::F32(a), Value::F32(b)) => a == b,
            // A bare float literal next to an f32 value is f32 in the source
            // types, so it rounds to f32 before the comparison.
            (Value::F32(a), Value::Float(b)) | (Value::Float(b), Value::F32(a)) => {
                *a == AsPrimitive::<f32>::as_(*b)
            }
            (Value::Int(a), Value::Float(b)) | (Value::Float(b), Value::Int(a)) => {
                AsPrimitive::<f64>::as_(*a) == *b
            }
            (Value::Char(a), Value::Char(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            // Snapshots, not held guards. Comparing a value with its own
            // clone sees the same mutex on both sides, and a guard held
            // across the recursion would relock a mutex the element methods
            // lock again. Both are instant deadlocks under parking_lot.
            (Value::Vec(a), Value::Vec(b)) | (Value::Tuple(a), Value::Tuple(b)) => {
                let a = a.lock().clone();
                let b = b.lock().clone();
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
            // Snapshot both field vectors before comparing. The old code held
            // both guards and then called `b.get`, which locks `b` again, so
            // any struct comparison with at least one field deadlocked. That
            // hung every script comparing two `PathBuf`s.
            (Value::Struct(a), Value::Struct(b)) => {
                a.name() == b.name() && {
                    let va = a.values.lock().clone();
                    let vb = b.values.lock().clone();
                    va.len() == vb.len()
                        && a.shape
                            .fields
                            .iter()
                            .zip(va.iter())
                            .all(|(k, v)| b.shape.slot(k).is_some_and(|i| v.eq_value(&vb[i])))
                }
            }
            (Value::Native(a), Value::Native(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// `Display`, the `{}` format.
    pub fn display(&self) -> String {
        match self {
            Value::Unit => "()".into(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::IntW(v, w) => w.decode(*v).to_string(),
            Value::Float(f) => format_float(*f),
            Value::F32(f) => f.to_string(),
            Value::Char(c) => c.to_string(),
            Value::Str(s) => s.to_string(),
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
            Value::IntW(v, w) => write!(out, "{}", w.decode(*v)).unwrap(),
            Value::Float(f) => out.push_str(&format_float_debug(*f)),
            Value::F32(f) => write!(out, "{f:?}").unwrap(),
            Value::Char(c) => write!(out, "{c:?}").unwrap(),
            Value::Str(s) => write!(out, "{:?}", &**s).unwrap(),
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
                for (i, v) in items.lock().iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    v.write_debug(out);
                }
                out.push(']');
            }
            Value::Tuple(items) => {
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
            Value::Map(map, kind) => {
                out.push('{');
                for (i, (k, v)) in map.lock().iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    k.write_debug(out);
                    if *kind == MapKind::Map {
                        out.push_str(": ");
                        v.write_debug(out);
                    }
                }
                out.push('}');
            }
            Value::Struct(s) => write_struct_debug(s, out),
            Value::Closure(_) => out.push_str("<closure>"),
            Value::Ref(reference) => match reference.get() {
                Some(value) => value.write_debug(out),
                None => out.push_str("<dangling reference>"),
            },
            Value::Native(n) => write!(out, "<{}>", n.lock().type_name()).unwrap(),
            Value::Enum { variant, data, .. } => {
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

/// The derived-Debug form of a struct.
fn write_struct_debug(s: &StructData, out: &mut String) {
    write!(out, "{}", super::resolver::bare(s.name())).unwrap();
    let values = s.values.lock();
    if values.is_empty() {
        return;
    }
    if s.shape
        .fields
        .iter()
        .enumerate()
        .all(|(i, f)| **f == i.to_string())
    {
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

impl MapKey {
    fn write_debug(&self, out: &mut String) {
        match self {
            MapKey::Bool(b) => write!(out, "{b}").unwrap(),
            MapKey::Int(i) => write!(out, "{i}").unwrap(),
            MapKey::Char(c) => write!(out, "{c:?}").unwrap(),
            MapKey::Str(s) => write!(out, "{:?}", &**s).unwrap(),
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            MapKey::Bool(b) => Value::Bool(*b),
            MapKey::Int(i) => Value::Int(*i),
            MapKey::Char(c) => Value::Char(*c),
            MapKey::Str(s) => Value::Str(s.clone()),
        }
    }
}

/// The host's Display and Debug are the target
/// semantics, so delegate instead of approximating them.
fn format_float(f: f64) -> String {
    f.to_string()
}

fn format_float_debug(f: f64) -> String {
    format!("{f:?}")
}
