//! The `Send + Sync` value model. `Arc` and `parking_lot::Mutex` back every
//! shared value, so it can move between worker threads and be shared by
//! concurrent tasks.

use num_traits::AsPrimitive;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use indexmap::IndexMap;
use parking_lot::Mutex;

use super::bytecode::Const;
use super::enum_def::{ERR, EnumDef, EnumKind, NONE, NOT_UNICODE, OK, OPTION, RESULT, SOME};
use super::native::Native;
use super::numeric::IntWidth;
pub use super::rs_str::RsStr;

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

/// Which wrapper type a `Value::Cell` models. The kind picks the method
/// surface and the debug rendering, the storage is the same shared slot.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    Rc,
    Arc,
    RefCell,
    Cell,
    Mutex,
    /// `tokio::sync::Mutex`, whose `lock` is awaited and answers the guard
    /// without a `Result` layer.
    TokioMutex,
}

impl CellKind {
    /// Rc and Arc share, they have no interior mutability of their own.
    pub fn is_shared_pointer(self) -> bool {
        matches!(self, CellKind::Rc | CellKind::Arc)
    }
}

pub enum ValueRef {
    VecElement {
        values: List,
        index: usize,
    },
    MapEntry {
        map: Map,
        key: MapKey,
    },
    StructField {
        data: Arc<StructData>,
        slot: usize,
    },
    /// The inside of a `Value::Cell`, handed out by `borrow_mut` and `lock`.
    CellSlot {
        slot: Arc<Mutex<Value>>,
    },
    /// A mutable borrow of the wrapped value's own storage, handed out by
    /// accessors like `as_object_mut`. Mutating through it must reach that
    /// storage, so the mutation split never applies. It has no anchor to
    /// assign a whole new value into, only in-place mutation goes through.
    Borrowed {
        value: Value,
    },
}

impl ValueRef {
    pub fn vec_element(values: List, index: usize) -> Self {
        Self::VecElement { values, index }
    }

    pub fn map_entry(map: Map, key: MapKey) -> Self {
        Self::MapEntry { map, key }
    }

    pub fn struct_field(data: Arc<StructData>, slot: usize) -> Self {
        Self::StructField { data, slot }
    }

    pub fn cell_slot(slot: Arc<Mutex<Value>>) -> Self {
        Self::CellSlot { slot }
    }

    pub fn borrowed(value: Value) -> Self {
        Self::Borrowed { value }
    }

    pub fn get(&self) -> Option<Value> {
        match self {
            Self::VecElement { values, index } => values.lock().get(*index).cloned(),
            Self::MapEntry { map, key } => map.lock().get(key).cloned(),
            Self::StructField { data, slot } => data.values.lock().get(*slot).cloned(),
            Self::CellSlot { slot } => Some(slot.lock().clone()),
            Self::Borrowed { value } => Some(value.clone()),
        }
    }

    /// Like `get`, but splits the referenced slot from value sharing first,
    /// so an in-place mutation of the returned value stays private to the
    /// slot. Used for mutating access through the reference.
    pub fn get_unique(&self) -> Option<Value> {
        let unique = |slot: Option<&mut Value>| {
            slot.map(|v| {
                v.make_unique();
                v.clone()
            })
        };
        match self {
            Self::VecElement { values, index } => unique(values.lock().get_mut(*index)),
            Self::MapEntry { map, key } => unique(map.lock().get_mut(key)),
            Self::StructField { data, slot } => unique(data.values.lock().get_mut(*slot)),
            Self::CellSlot { slot } => unique(Some(&mut *slot.lock())),
            // The wrapped value IS the borrowed storage, splitting it would
            // disconnect the mutation from the borrow's referent.
            Self::Borrowed { value } => Some(value.clone()),
        }
    }

    /// Run `f` on the referenced slot under its lock, one atomic
    /// read-modify-write for a fused compound assignment. `None` for a
    /// dangling reference and for a `Borrowed` view, which has no
    /// assignable slot, the caller then runs the unfused sequence and gets
    /// its exact error. `f` must not run user code or lock other values,
    /// the referent's lock is held across it.
    pub fn update<T>(&self, f: impl FnOnce(&mut Value) -> T) -> Option<T> {
        match self {
            Self::VecElement { values, index } => values.lock().get_mut(*index).map(f),
            Self::MapEntry { map, key } => map.lock().get_mut(key).map(f),
            Self::StructField { data, slot } => data.values.lock().get_mut(*slot).map(f),
            Self::CellSlot { slot } => Some(f(&mut slot.lock())),
            Self::Borrowed { .. } => None,
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
            Self::StructField { data, slot } => {
                let mut values = data.values.lock();
                let Some(target) = values.get_mut(*slot) else {
                    return false;
                };
                *target = value;
                true
            }
            Self::CellSlot { slot } => {
                *slot.lock() = value;
                true
            }
            Self::Borrowed { .. } => false,
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
    /// A 128-bit integer, i128 exactly or u128 as reinterpreted bits.
    Big(i128, IntWidth),
    Float(f64),
    /// A real f32, kept at f32 precision, mirroring `Value::F32`.
    F32(f32),
    Char(char),
    Str(RsStr),
    Vec(List),
    Map(Map, MapKind),
    Tuple(List),
    Struct(Arc<StructData>),
    /// The payload shares the list storage shape, so a `&mut` binding into
    /// a payload slot is an addressable element reference.
    Enum {
        def: Arc<EnumDef>,
        variant: u16,
        data: List,
    },
    Range {
        start: i64,
        end: i64,
        inclusive: bool,
    },
    Closure(Arc<ClosureData>),
    Ref(Arc<ValueRef>),
    /// A real shared cell: `Rc`, `Arc`, `RefCell`, `Cell`, or `Mutex`.
    /// Cloning shares the slot on purpose, these are the types real Rust
    /// uses when sharing is the point, so `make_unique` leaves them alone.
    Cell(CellKind, Arc<Mutex<Value>>),
    Native(Arc<Mutex<Native>>),
}

/// Hashable map key, the subset of values that may be keys. The manual
/// `Hash` pins the exact bytes each variant feeds the hasher, so the
/// borrowed `StrKey` below can probe a map without building an owned key.
#[derive(Clone, PartialEq, Eq)]
pub enum MapKey {
    Bool(bool),
    Int(i64),
    Char(char),
    Str(RsStr),
}

impl Hash for MapKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            MapKey::Bool(b) => {
                state.write_u8(0);
                b.hash(state);
            }
            MapKey::Int(i) => {
                state.write_u8(1);
                i.hash(state);
            }
            MapKey::Char(c) => {
                state.write_u8(2);
                c.hash(state);
            }
            MapKey::Str(s) => {
                state.write_u8(3);
                (**s).hash(state);
            }
        }
    }
}

/// A borrowed string key for map probes that hold the slice but not an
/// owned `RsStr`, hashing and comparing exactly like `MapKey::Str`.
pub struct StrKey<'a>(pub &'a str);

impl Hash for StrKey<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u8(3);
        self.0.hash(state);
    }
}

impl indexmap::Equivalent<MapKey> for StrKey<'_> {
    fn equivalent(&self, key: &MapKey) -> bool {
        matches!(key, MapKey::Str(s) if **s == *self.0)
    }
}

impl Value {
    pub fn str(s: impl Into<RsStr>) -> Value {
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
        Value::enum_of(&OPTION, SOME, vec![v])
    }

    /// An enum value with its payload in fresh list storage.
    pub fn enum_of(def: &Arc<EnumDef>, variant: u16, data: Vec<Value>) -> Value {
        Value::Enum {
            def: def.clone(),
            variant,
            data: Arc::new(Mutex::new(data)),
        }
    }

    /// An enum value by variant name, for the bridges that receive the
    /// name from a script or from a crate's `Debug` output. None when the
    /// definition has no such variant.
    pub fn enum_named(def: &Arc<EnumDef>, variant: &str, data: Vec<Value>) -> Option<Value> {
        Some(Value::enum_of(def, def.variant_index(variant)?, data))
    }

    pub fn none() -> Value {
        Value::enum_of(&OPTION, NONE, Vec::new())
    }

    /// Whether this is a variant of a builtin enum, `is_variant(EnumKind::Option, SOME)`.
    pub fn is_variant(&self, kind: EnumKind, index: u16) -> bool {
        matches!(self, Value::Enum { def, variant, .. } if def.kind == kind && *variant == index)
    }

    /// Whether this is an enum of the given builtin kind, any variant.
    pub fn is_enum_kind(&self, kind: EnumKind) -> bool {
        matches!(self, Value::Enum { def, .. } if def.kind == kind)
    }

    /// True for `Option::None`, used to keep a null json value as None rather
    /// than wrapping it in Some when filling an Option struct field.
    pub fn is_none_value(&self) -> bool {
        self.is_variant(EnumKind::Option, NONE)
    }

    /// The payload of an `Option::Some`, or None for `Option::None` and
    /// for anything that is not an Option.
    pub fn some_payload(&self) -> Option<Value> {
        match self {
            Value::Enum { def, variant, data }
                if def.kind == EnumKind::Option && *variant == SOME =>
            {
                Some(data.lock().first().cloned().unwrap_or(Value::Unit))
            }
            _ => None,
        }
    }

    /// The payload of a `Some` or an `Ok`, the value a flatten keeps.
    pub fn success_payload(&self) -> Option<Value> {
        match self {
            Value::Enum { def, variant, data }
                if (def.kind == EnumKind::Option && *variant == SOME)
                    || (def.kind == EnumKind::Result && *variant == OK) =>
            {
                Some(data.lock().first().cloned().unwrap_or(Value::Unit))
            }
            _ => None,
        }
    }

    pub fn ok(v: Value) -> Value {
        Value::enum_of(&RESULT, OK, vec![v])
    }

    pub fn err(v: Value) -> Value {
        Value::enum_of(&RESULT, ERR, vec![v])
    }

    pub fn is_truthy(&self) -> bool {
        matches!(self, Value::Bool(true))
    }

    /// A fresh value of the same shape as this one, standing in for
    /// `T::default()` where the runtime has no type: `mem::take` and
    /// `RefCell::take`.
    pub(super) fn default_like(&self) -> Value {
        match self {
            Value::Bool(_) => Value::Bool(false),
            Value::Int(_) => Value::Int(0),
            Value::IntW(_, w) => Value::IntW(0, *w),
            Value::Big(_, w) => Value::Big(0, *w),
            Value::Float(_) => Value::Float(0.0),
            Value::F32(_) => Value::F32(0.0),
            Value::Char(_) => Value::Char('\0'),
            Value::Str(_) => Value::str(""),
            Value::Vec(_) => Value::vec(Vec::new()),
            Value::Map(_, MapKind::Map) => Value::map(),
            Value::Map(_, MapKind::Set) => Value::set(),
            Value::Enum { def, .. } if def.kind == EnumKind::Option => Value::none(),
            _ => Value::Unit,
        }
    }

    /// Split this value from any sharing so an in-place mutation stays
    /// private: when the backing storage has another holder, replace it with
    /// a fresh copy. One level deep on purpose, nested values split at their
    /// own mutable access, so a chain of these along an access path behaves
    /// like a deep copy paid lazily.
    ///
    /// Scalars and strings need nothing, a string splits inside its own
    /// mutating methods via `Arc::make_mut`. Native handles and closures
    /// keep their identity, sharing is their meaning.
    pub(super) fn make_unique(&mut self) {
        match self {
            Value::Vec(list) | Value::Tuple(list) => {
                if Arc::strong_count(list) > 1 {
                    let copy = list.lock().clone();
                    *list = Arc::new(Mutex::new(copy));
                }
            }
            Value::Map(map, _) => {
                if Arc::strong_count(map) > 1 {
                    let copy = map.lock().clone();
                    *map = Arc::new(Mutex::new(copy));
                }
            }
            Value::Struct(data) => {
                if Arc::strong_count(data) > 1 {
                    let values = data.values.lock().clone();
                    *data = Arc::new(StructData {
                        shape: data.shape.clone(),
                        values: Mutex::new(values),
                    });
                }
            }
            Value::Enum { data, .. } if Arc::strong_count(data) > 1 => {
                let copy = data.lock().clone();
                *data = Arc::new(Mutex::new(copy));
            }
            _ => {}
        }
    }

    pub fn from_const(c: &Const) -> Value {
        match c {
            Const::Big(v, w) => Value::Big(*v, *w),
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
            Value::Big(v, IntWidth::I128) => Some((*v, IntWidth::I128)),
            // A u128 fits the i128 pipeline only while its top bit is clear.
            // Bigger values answer through the dedicated big paths instead.
            Value::Big(v, IntWidth::U128) if *v >= 0 => Some((*v, IntWidth::U128)),
            _ => None,
        }
    }

    /// Build an integer of the given width from an in-range value.
    pub(super) fn int_of_width(value: i128, width: IntWidth) -> Value {
        match width {
            IntWidth::I64 => Value::Int(i64::try_from(value).expect("truncated to width")),
            IntWidth::I128 | IntWidth::U128 => Value::Big(value, width),
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
            Value::Int(_) | Value::IntW(..) | Value::Big(..) => "integer",
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
            Value::Cell(kind, _) => match kind {
                CellKind::Rc => "Rc",
                CellKind::Arc => "Arc",
                CellKind::RefCell => "RefCell",
                CellKind::Cell => "Cell",
                CellKind::Mutex | CellKind::TokioMutex => "Mutex",
            },
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
        // Cells compare by content, the way Rc and RefCell equality does.
        // Snapshots, not held guards, comparing a cell with itself would
        // relock its own mutex.
        if let Value::Cell(_, slot) = self {
            let inner = slot.lock().clone();
            return inner.eq_value(other);
        }
        if let Value::Cell(_, slot) = other {
            let inner = slot.lock().clone();
            return self.eq_value(&inner);
        }
        match (self, other) {
            (Value::Unit, Value::Unit) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::IntW(..), Value::Int(_) | Value::IntW(..))
            | (Value::Int(_), Value::IntW(..)) => {
                self.int_parts().map(|(a, _)| a) == other.int_parts().map(|(b, _)| b)
            }
            (Value::Big(a, wa), Value::Big(b, wb)) => a == b && wa == wb,
            (Value::Big(..), Value::Int(_)) | (Value::Int(_), Value::Big(..)) => {
                match (self.int_parts(), other.int_parts()) {
                    (Some((a, _)), Some((b, _))) => a == b,
                    // One side is a u128 past the i128 range, the other an
                    // i64, so they cannot be equal.
                    _ => false,
                }
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
                    def: ea,
                    variant: va,
                    data: da,
                },
                Value::Enum {
                    def: eb,
                    variant: vb,
                    data: db,
                },
            ) => {
                // Snapshots, not held guards, see the container arms above.
                let da = da.lock().clone();
                let db = db.lock().clone();
                EnumDef::same(ea, eb)
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
            Value::Big(v, w) => big_text(*v, *w),
            Value::Float(f) => format_float(*f),
            Value::F32(f) => f.to_string(),
            Value::Char(c) => c.to_string(),
            Value::Str(s) => s.to_string(),
            // Rc and Arc pass Display through to their content.
            Value::Cell(kind, slot) if kind.is_shared_pointer() => {
                let inner = slot.lock().clone();
                inner.display()
            }
            // A reference displays what it points at, `{}` through a `&mut`
            // borrow or a lock guard formats the content.
            Value::Ref(reference) => match reference.get() {
                Some(value) => value.display(),
                None => "<dangling reference>".to_string(),
            },
            Value::Native(n) => match &*n.lock() {
                Native::IoErr { display, .. } | Native::JoinErr { display, .. } => display.clone(),
                other => format!("<{}>", other.type_name()),
            },
            // `std::env::VarError` implements `Display` in real Rust, and
            // its text is what scripts print with `{e}`.
            Value::Enum { def, variant, data } if def.kind == EnumKind::VarError => {
                if *variant == NOT_UNICODE {
                    let payload = data.lock().first().map(Value::display).unwrap_or_default();
                    format!("environment variable was not valid unicode: {payload:?}")
                } else {
                    "environment variable not found".to_string()
                }
            }
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
            Value::Big(v, w) => out.push_str(&big_text(*v, *w)),
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
            Value::Cell(kind, slot) => write_cell_debug(*kind, slot, out),
            Value::Native(n) => {
                if let Native::IoErr { debug, .. } | Native::JoinErr { debug, .. } = &*n.lock() {
                    out.push_str(debug);
                } else {
                    write!(out, "<{}>", n.lock().type_name()).unwrap();
                }
            }
            Value::Enum { def, variant, data } => {
                out.push_str(def.variant_name(*variant));
                let data = data.lock().clone();
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

/// A shared cell's debug form, each wrapper rendering the way its real
/// derive does. The slot is snapshotted, not held, so a nested read cannot
/// relock it.
fn write_cell_debug(kind: CellKind, slot: &Arc<Mutex<Value>>, out: &mut String) {
    let inner = slot.lock().clone();
    match kind {
        CellKind::Rc | CellKind::Arc => inner.write_debug(out),
        CellKind::RefCell => {
            out.push_str("RefCell { value: ");
            inner.write_debug(out);
            out.push_str(" }");
        }
        CellKind::Cell => {
            out.push_str("Cell { value: ");
            inner.write_debug(out);
            out.push_str(" }");
        }
        CellKind::Mutex => {
            out.push_str("Mutex { data: ");
            inner.write_debug(out);
            out.push_str(", poisoned: false, .. }");
        }
        CellKind::TokioMutex => {
            out.push_str("Mutex { data: ");
            inner.write_debug(out);
            out.push_str(" }");
        }
    }
}

/// A 128-bit value as text, u128 through its reinterpreted bits.
fn big_text(v: i128, w: IntWidth) -> String {
    if w == IntWidth::U128 {
        v.cast_unsigned().to_string()
    } else {
        v.to_string()
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
