use std::cell::{Cell, RefCell};
use std::fmt::{self, Write};
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::ptr;
use std::rc::Rc;

use compact_str::CompactString;
use indexmap::{Equivalent, IndexMap};
use rustc_hash::{FxBuildHasher, FxHasher};

use super::bytecode::Const;
use super::native::Native;

/// Interpreter string. The buffer is immutable while the `Rc` is shared, so
/// clones and `to_string` are refcount bumps. Mutation goes through
/// `Value::str_make_mut`, which copies first when another handle exists. That
/// matches real `String` semantics, where a clone never sees later edits to
/// the original. The hash of the bytes is cached after the first map use,
/// the same trick CPython uses for str objects.
pub struct RStr {
    /// Cached key hash of the bytes. 0 means not computed yet.
    hash: Cell<u64>,
    /// Inline storage up to 24 bytes, so short strings cost one allocation
    /// for the `Rc` and none for the bytes.
    s: CompactString,
}

impl RStr {
    pub fn new(s: impl Into<CompactString>) -> Rc<RStr> {
        Rc::new(RStr {
            hash: Cell::new(0),
            s: s.into(),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.s
    }

    /// The cached key hash, computed on first use.
    pub fn key_hash(&self) -> u64 {
        let h = self.hash.get();
        if h != 0 {
            return h;
        }
        let h = str_hash(&self.s);
        self.hash.set(h);
        h
    }
}

/// Hash used for string map keys. Reserves 0 as the "not cached" sentinel.
fn str_hash(s: &str) -> u64 {
    let mut h = FxHasher::default();
    s.hash(&mut h);
    let v = h.finish();
    if v == 0 { 1 } else { v }
}

impl Deref for RStr {
    type Target = str;

    fn deref(&self) -> &str {
        &self.s
    }
}

impl PartialEq for RStr {
    fn eq(&self, other: &RStr) -> bool {
        self.s == other.s
    }
}

impl Eq for RStr {}

impl fmt::Display for RStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.s)
    }
}

impl fmt::Debug for RStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.s)
    }
}

/// Field layout of a struct, shared by every instance built from the same
/// site. Instances then carry a plain `Vec<Value>` in this order, so a field
/// read is a short name scan plus an index, not a hash probe, and building an
/// instance allocates no map.
pub struct StructShape {
    pub name: Rc<str>,
    pub fields: Vec<Rc<str>>,
    /// One entry per field, its `#[serde(rename = "..")]` name if any. Empty
    /// when the struct has no renamed fields. Read when serializing to json so
    /// the output key matches serde, the same names deserialize already honors.
    pub renames: Vec<Option<Rc<str>>>,
}

impl StructShape {
    pub fn new(name: impl Into<Rc<str>>, fields: Vec<Rc<str>>) -> Rc<StructShape> {
        Rc::new(StructShape {
            name: name.into(),
            fields,
            renames: Vec::new(),
        })
    }

    pub fn with_renames(
        name: impl Into<Rc<str>>,
        fields: Vec<Rc<str>>,
        renames: Vec<Option<Rc<str>>>,
    ) -> Rc<StructShape> {
        Rc::new(StructShape {
            name: name.into(),
            fields,
            renames,
        })
    }

    /// Slot index of a field. Structs have a handful of fields, so a linear
    /// scan beats hashing.
    pub fn slot(&self, field: &str) -> Option<usize> {
        self.fields.iter().position(|f| &**f == field)
    }
}

/// A struct instance: its shape plus one value per field, in shape order.
pub struct StructData {
    pub shape: Rc<StructShape>,
    pub values: RefCell<Vec<Value>>,
}

impl StructData {
    pub fn name(&self) -> &Rc<str> {
        &self.shape.name
    }

    pub fn get(&self, field: &str) -> Option<Value> {
        self.shape
            .slot(field)
            .map(|i| self.values.borrow()[i].clone())
    }

    /// Write a field that exists in the shape. False when it does not.
    pub fn set(&self, field: &str, v: Value) -> bool {
        match self.shape.slot(field) {
            Some(i) => {
                self.values.borrow_mut()[i] = v;
                true
            }
            None => false,
        }
    }
}

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
    Str(Rc<RStr>),
    Vec(Rc<RefCell<Vec<Value>>>),
    Map(Rc<RefCell<Map>>),
    Tuple(Rc<RefCell<Vec<Value>>>),
    /// Struct instance. Named fields, or positional for tuple structs.
    Struct(Rc<StructData>),
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

/// Hashable key for maps. Only a subset of values can be keys. String keys
/// share the value's buffer, so building a key from a string never copies.
#[derive(Clone)]
pub enum MapKey {
    Bool(bool),
    Int(i64),
    Char(char),
    Str(Rc<RStr>),
}

impl PartialEq for MapKey {
    fn eq(&self, other: &MapKey) -> bool {
        match (self, other) {
            (MapKey::Bool(a), MapKey::Bool(b)) => a == b,
            (MapKey::Int(a), MapKey::Int(b)) => a == b,
            (MapKey::Char(a), MapKey::Char(b)) => a == b,
            (MapKey::Str(a), MapKey::Str(b)) => Rc::ptr_eq(a, b) || a == b,
            _ => false,
        }
    }
}

impl Eq for MapKey {}

/// Hashes must not include the variant tag so a borrowed `KeyRef` lookup and a
/// stored `MapKey` land in the same bucket. Cross-variant collisions are fine,
/// equality still separates them. String keys feed the cached `key_hash` into
/// the state, and every other string-key hasher below must do the same, or
/// lookups miss.
impl Hash for MapKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            MapKey::Bool(b) => b.hash(state),
            MapKey::Int(i) => i.hash(state),
            MapKey::Char(c) => c.hash(state),
            MapKey::Str(s) => state.write_u64(s.key_hash()),
        }
    }
}

/// Borrowed view of a value used as a map key, so `get` and `contains_key` do
/// not clone the key string. `Interned` reuses the cached hash of the value's
/// buffer, `Str` is for plain `&str` callers and hashes the bytes.
pub enum KeyRef<'a> {
    Bool(bool),
    Int(i64),
    Char(char),
    Str(&'a str),
    Interned(&'a RStr),
}

impl Hash for KeyRef<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            KeyRef::Bool(b) => b.hash(state),
            KeyRef::Int(i) => i.hash(state),
            KeyRef::Char(c) => c.hash(state),
            KeyRef::Str(s) => state.write_u64(str_hash(s)),
            KeyRef::Interned(s) => state.write_u64(s.key_hash()),
        }
    }
}

impl Equivalent<MapKey> for KeyRef<'_> {
    fn equivalent(&self, key: &MapKey) -> bool {
        match (self, key) {
            (KeyRef::Bool(a), MapKey::Bool(b)) => a == b,
            (KeyRef::Int(a), MapKey::Int(b)) => a == b,
            (KeyRef::Char(a), MapKey::Char(b)) => a == b,
            (KeyRef::Str(a), MapKey::Str(b)) => *a == &***b,
            (KeyRef::Interned(a), MapKey::Str(b)) => ptr::eq(*a, Rc::as_ptr(b)) || *a == &**b,
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
    pub fn str(s: impl Into<CompactString>) -> Value {
        Value::Str(RStr::new(s))
    }

    /// Materialize a chunk's literal constant into a runtime value.
    pub fn from_const(c: &Const) -> Value {
        match c {
            Const::Float(f) => Value::Float(*f),
            Const::Char(ch) => Value::Char(*ch),
            Const::Str(s) => Value::str(&**s),
            Const::Bytes(bytes) => {
                Value::vec(bytes.iter().map(|&b| Value::Int(b as i64)).collect())
            }
        }
    }

    /// Mutable access to a string buffer. Copies first when the handle is
    /// shared, so other holders keep the old contents, exactly like editing
    /// one `String` clone in real Rust. Resets the cached hash.
    pub fn str_make_mut(rc: &mut Rc<RStr>) -> &mut CompactString {
        if Rc::get_mut(rc).is_none() {
            *rc = RStr::new(rc.s.clone());
        }
        let inner = Rc::get_mut(rc).unwrap();
        inner.hash.set(0);
        &mut inner.s
    }

    pub fn vec(items: Vec<Value>) -> Value {
        Value::Vec(Rc::new(RefCell::new(items)))
    }

    pub fn structure(shape: Rc<StructShape>, values: Vec<Value>) -> Value {
        Value::Struct(Rc::new(StructData {
            shape,
            values: RefCell::new(values),
        }))
    }

    /// One-off struct built by a bridge, shape and instance in one go.
    pub fn struct_of(
        name: impl Into<Rc<str>>,
        pairs: impl IntoIterator<Item = (Rc<str>, Value)>,
    ) -> Value {
        let (fields, values) = pairs.into_iter().unzip();
        Value::structure(StructShape::new(name, fields), values)
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

    /// True for `Option::None`, used to keep a null json value as None rather
    /// than wrapping it in Some when filling an Option struct field.
    pub fn is_none_value(&self) -> bool {
        matches!(self, Value::Enum { enum_name, variant, .. }
            if &**enum_name == "Option" && &**variant == "None")
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
            Value::Struct(_) => "struct",
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
            Value::Char(c) => MapKey::Char(c),
            Value::Str(s) => MapKey::Str(s),
            _ => return None,
        })
    }

    /// Borrowed key view of this value for lookups that must not clone.
    /// None when the value cannot be a key.
    pub fn key_ref(&self) -> Option<KeyRef<'_>> {
        Some(match self {
            Value::Bool(b) => KeyRef::Bool(*b),
            Value::Int(i) => KeyRef::Int(*i),
            Value::Char(c) => KeyRef::Char(*c),
            Value::Str(s) => KeyRef::Interned(s),
            _ => return None,
        })
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
            (Value::Str(a), Value::Str(b)) => Rc::ptr_eq(a, b) || a == b,
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
            (Value::Struct(a), Value::Struct(b)) => {
                a.name() == b.name() && {
                    let (va, vb) = (a.values.borrow(), b.values.borrow());
                    va.len() == vb.len()
                        && a.shape
                            .fields
                            .iter()
                            .zip(va.iter())
                            .all(|(k, v)| b.get(k).map(|o| v.eq_value(&o)).unwrap_or(false))
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
            Value::Str(s) => s.s.as_str().to_string(),
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
            Value::Struct(s) => {
                // Canonical names print bare, like the compiler's Debug derive.
                write!(out, "{}", super::resolver::bare(s.name())).unwrap();
                let values = s.values.borrow();
                if values.is_empty() {
                    return;
                }
                // Tuple structs carry positional field names and print in
                // paren form, matching the derived Debug output.
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
            Value::Closure(_) => out.push_str("<closure>"),
            Value::Native(n) => write!(out, "<{}>", n.borrow().type_name()).unwrap(),
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

impl MapKey {
    fn write_debug(&self, out: &mut String) {
        match self {
            MapKey::Bool(b) => write!(out, "{b}").unwrap(),
            MapKey::Int(i) => write!(out, "{i}").unwrap(),
            MapKey::Char(c) => write!(out, "{c:?}").unwrap(),
            MapKey::Str(s) => write!(out, "{:?}", &***s).unwrap(),
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

fn format_float(f: f64) -> String {
    if f == f.trunc() && f.is_finite() {
        format!("{f:.0}")
    } else {
        f.to_string()
    }
}
