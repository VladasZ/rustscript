//! The value model. `Send + Sync`, every shared value sits behind `Arc` and `parking_lot::Mutex`.

use num_traits::AsPrimitive;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use indexmap::IndexMap;
use parking_lot::Mutex;
use rustc_hash::FxBuildHasher;

use super::bytecode::Const;
use super::enum_def::{ERR, EnumDef, EnumKind, NONE, NOT_UNICODE, OK, OPTION, RESULT, SOME};
use super::native::Native;
use super::numeric::IntWidth;
pub use super::rs_str::RsStr;

pub type List = Arc<Mutex<Vec<Value>>>;
/// Insertion ordered like every script map, hashed with Fx, `SipHash` was the top of the
/// `word_count` profile.
pub type MapStore = IndexMap<MapKey, Value, FxBuildHasher>;
pub type Map = Arc<Mutex<MapStore>>;

/// A set is a map with Unit values. The kind makes iteration yield elements and picks the set
/// half of the methods.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MapKind {
    Map,
    Set,
}

/// Picks the method surface and the debug rendering.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    Rc,
    Arc,
    RefCell,
    Cell,
    Mutex,
    /// `tokio::sync::Mutex`, `lock` is awaited and there is no `Result` layer
    TokioMutex,
}

impl CellKind {
    /// `Rc` and `Arc` have no interior mutability on their own
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
    /// from `borrow_mut` and `lock`
    CellSlot {
        slot: Arc<Mutex<Value>>,
    },
    /// From accessors like `as_object_mut`. No mutation split here, only in place mutation goes through.
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

    /// One atomic read-modify-write under the slot lock. `None` for a dangling reference or a
    /// `Borrowed` view, then the caller runs the unfused sequence. `f` must not run user code or
    /// lock other values.
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

/// Runtime and bytecode share 1 definition.
pub use super::bytecode::StructShape;

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

#[derive(Clone, Default)]
pub enum Value {
    #[default]
    Unit,
    Bool(bool),
    Int(i64),
    /// any width other than i64, storage form is in `numeric`
    IntW(i64, IntWidth),
    /// i128 as is, u128 as reinterpreted bits
    Big(i128, IntWidth),
    Float(f64),
    /// kept at f32 precision
    F32(f32),
    Char(char),
    Str(RsStr),
    Vec(List),
    Map(Map, MapKind),
    Tuple(List),
    Struct(Arc<StructData>),
    /// Payload uses the list storage shape, so a `&mut` binding into it is an element reference.
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
    /// A real shared cell. Cloning shares on purpose.
    Cell(CellKind, Arc<Mutex<Value>>),
    Native(Arc<Mutex<Native>>),
}

/// Manual `Hash` so the exact bytes per variant are fixed.
#[derive(Clone)]
pub enum MapKey {
    Bool(bool),
    Int(i64),
    /// hashes and compares like `Int`, 1 real map never mixes widths
    Wide(i64, IntWidth),
    Char(char),
    Str(RsStr),
    Unit,
    Tuple(Vec<MapKey>),
    Opt(Option<Box<MapKey>>),
    Vec(Vec<MapKey>),
    /// the shape is kept to rebuild the value
    Struct(Arc<StructShape>, Vec<MapKey>),
    Enum(Arc<EnumDef>, u16, Vec<MapKey>),
}

impl PartialEq for MapKey {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (MapKey::Bool(a), MapKey::Bool(b)) => a == b,
            (MapKey::Int(a) | MapKey::Wide(a, _), MapKey::Int(b) | MapKey::Wide(b, _)) => a == b,
            (MapKey::Char(a), MapKey::Char(b)) => a == b,
            (MapKey::Str(a), MapKey::Str(b)) => a == b,
            (MapKey::Unit, MapKey::Unit) => true,
            (MapKey::Tuple(a), MapKey::Tuple(b)) | (MapKey::Vec(a), MapKey::Vec(b)) => a == b,
            (MapKey::Opt(a), MapKey::Opt(b)) => a == b,
            (MapKey::Struct(sa, a), MapKey::Struct(sb, b)) => sa.name == sb.name && a == b,
            (MapKey::Enum(da, va, a), MapKey::Enum(db, vb, b)) => {
                da.name == db.name && va == vb && a == b
            }
            _ => false,
        }
    }
}

impl Eq for MapKey {}

impl Hash for MapKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            MapKey::Bool(b) => {
                state.write_u8(0);
                b.hash(state);
            }
            MapKey::Int(i) | MapKey::Wide(i, _) => {
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
            MapKey::Unit => state.write_u8(4),
            MapKey::Tuple(items) => {
                state.write_u8(5);
                items.hash(state);
            }
            MapKey::Opt(inner) => {
                state.write_u8(6);
                inner.hash(state);
            }
            MapKey::Vec(items) => {
                state.write_u8(7);
                items.hash(state);
            }
            MapKey::Struct(shape, fields) => {
                state.write_u8(8);
                shape.name.hash(state);
                fields.hash(state);
            }
            MapKey::Enum(def, variant, payload) => {
                state.write_u8(9);
                def.name.hash(state);
                variant.hash(state);
                payload.hash(state);
            }
        }
    }
}

fn keys_of(values: &[Value]) -> Option<Vec<MapKey>> {
    values.iter().map(Value::as_key).collect()
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

    pub fn map_of(map: MapStore) -> Value {
        Value::Map(Arc::new(Mutex::new(map)), MapKind::Map)
    }

    pub fn set() -> Value {
        Value::Map(Arc::new(Mutex::new(IndexMap::default())), MapKind::Set)
    }

    pub fn set_of(map: MapStore) -> Value {
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

    pub fn enum_of(def: &Arc<EnumDef>, variant: u16, data: Vec<Value>) -> Value {
        Value::Enum {
            def: def.clone(),
            variant,
            data: Arc::new(Mutex::new(data)),
        }
    }

    /// By variant name, for bridges that get the name from a script or from a crate's `Debug` output.
    pub fn enum_named(def: &Arc<EnumDef>, variant: &str, data: Vec<Value>) -> Option<Value> {
        Some(Value::enum_of(def, def.variant_index(variant)?, data))
    }

    pub fn none() -> Value {
        Value::enum_of(&OPTION, NONE, Vec::new())
    }

    /// `is_variant(EnumKind::Option, SOME)`
    pub fn is_variant(&self, kind: EnumKind, index: u16) -> bool {
        matches!(self, Value::Enum { def, variant, .. } if def.kind == kind && *variant == index)
    }

    pub fn is_enum_kind(&self, kind: EnumKind) -> bool {
        matches!(self, Value::Enum { def, .. } if def.kind == kind)
    }

    /// To keep a json null as None when filling an Option field.
    pub fn is_none_value(&self) -> bool {
        self.is_variant(EnumKind::Option, NONE)
    }

    pub fn some_payload(&self) -> Option<Value> {
        match self {
            Value::Enum { def, variant, data }
                if def.kind == EnumKind::Option && *variant == SOME =>
            {
                data.lock().first().cloned()
            }
            _ => None,
        }
    }

    /// What a flatten keeps.
    pub fn success_payload(&self) -> Option<Value> {
        match self {
            Value::Enum { def, variant, data }
                if (def.kind == EnumKind::Option && *variant == SOME)
                    || (def.kind == EnumKind::Result && *variant == OK) =>
            {
                data.lock().first().cloned()
            }
            _ => None,
        }
    }

    /// An empty payload is an interpreter bug.
    pub fn payload(data: &List) -> Result<Value> {
        data.lock()
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("enum payload is missing"))
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

    /// Stands in for `T::default()` in `mem::take` and `RefCell::take`.
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

    /// What `clone()` and a `Copy` read do. Composites copy all the way down, so the copy and the
    /// original never share storage. `Rc` and `Arc` share on purpose. Strings are copy on write
    /// inside their own methods, so the handle is enough. A reference stays a reference, `&T` is
    /// `Copy`. Natives and closures keep their identity.
    pub fn deep_clone(&self) -> Value {
        let items = |list: &Mutex<Vec<Value>>| list.lock().iter().map(Value::deep_clone).collect();
        match self {
            Value::Vec(list) => Value::vec(items(list)),
            Value::Tuple(list) => Value::tuple(items(list)),
            Value::Map(map, kind) => {
                let copy = map
                    .lock()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.deep_clone()))
                    .collect();
                Value::Map(Arc::new(Mutex::new(copy)), *kind)
            }
            Value::Struct(data) => Value::structure(data.shape.clone(), items(&data.values)),
            Value::Enum { def, variant, data } => Value::enum_of(def, *variant, items(data)),
            Value::Cell(kind, slot) if !kind.is_shared_pointer() => {
                let inner = slot.lock().deep_clone();
                Value::Cell(*kind, Arc::new(Mutex::new(inner)))
            }
            other => other.clone(),
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

    pub(super) fn int_parts(&self) -> Option<(i128, IntWidth)> {
        match self {
            Value::Int(i) => Some((i128::from(*i), IntWidth::I64)),
            Value::IntW(v, w) => Some((w.decode(*v), *w)),
            Value::Big(v, IntWidth::I128) => Some((*v, IntWidth::I128)),
            // a u128 fits the i128 pipeline only while its top bit is clear
            Value::Big(v, IntWidth::U128) if *v >= 0 => Some((*v, IntWidth::U128)),
            _ => None,
        }
    }

    pub(super) fn int_of_width(value: i128, width: IntWidth) -> Value {
        match width {
            IntWidth::I64 => Value::Int(i64::try_from(value).expect("truncated to width")),
            IntWidth::I128 | IntWidth::U128 => Value::Big(value, width),
            other => Value::IntW(other.encode(value), other),
        }
    }

    pub(super) fn untag_int(&self) -> Option<i64> {
        match self {
            Value::IntW(v, w) => i64::try_from(w.decode(*v)).ok(),
            _ => None,
        }
    }

    /// The i64 or f64 image for the older bridge surface that has no widths. A u64 past
    /// `i64::MAX` saturates. None when not tagged.
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

    /// Holds no heap handle, so dropping it does nothing.
    #[inline]
    pub fn is_plain(&self) -> bool {
        matches!(
            self,
            Value::Unit
                | Value::Bool(_)
                | Value::Int(_)
                | Value::IntW(..)
                | Value::Big(..)
                | Value::Float(_)
                | Value::F32(_)
                | Value::Char(_)
                | Value::Range { .. }
        )
    }

    /// Releases a heap handle. A plain value stays, it is not worth the write.
    #[inline]
    pub fn release(&mut self) {
        if !self.is_plain() {
            *self = Value::Unit;
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
            Value::IntW(v, w) => MapKey::Wide(*v, *w),
            Value::Char(c) => MapKey::Char(*c),
            Value::Str(s) => MapKey::Str(s.clone()),
            Value::Unit => MapKey::Unit,
            Value::Tuple(items) => MapKey::Tuple(keys_of(&items.lock())?),
            Value::Vec(items) => MapKey::Vec(keys_of(&items.lock())?),
            Value::Struct(s) => MapKey::Struct(s.shape.clone(), keys_of(&s.values.lock())?),
            Value::Enum { def, variant, data } if def.kind == EnumKind::Option => {
                if *variant == SOME {
                    MapKey::Opt(Some(Box::new(data.lock().first()?.as_key()?)))
                } else {
                    MapKey::Opt(None)
                }
            }
            Value::Enum { def, variant, data } => {
                MapKey::Enum(def.clone(), *variant, keys_of(&data.lock())?)
            }
            Value::Ref(reference) => return reference.get()?.as_key(),
            _ => return None,
        })
    }

    /// Strings hand over their buffer, no copy.
    pub fn into_key(self) -> Option<MapKey> {
        Some(match self {
            Value::Bool(b) => MapKey::Bool(b),
            Value::Int(i) => MapKey::Int(i),
            Value::IntW(v, w) => MapKey::Wide(v, w),
            Value::Char(c) => MapKey::Char(c),
            Value::Str(s) => MapKey::Str(s),
            other => return other.as_key(),
        })
    }

    pub fn eq_value(&self, other: &Value) -> bool {
        if let Value::Ref(reference) = self {
            return reference.get().is_some_and(|value| value.eq_value(other));
        }
        if let Value::Ref(reference) = other {
            return reference.get().is_some_and(|value| self.eq_value(&value));
        }
        // Cells compare by content. Use snapshots, not held guards, comparing a cell with itself
        // would relock its own mutex.
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
                    // a u128 past the i128 range can't equal an i64
                    _ => false,
                }
            }
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::F32(a), Value::F32(b)) => a == b,
            // a bare float literal next to an f32 is f32, so round first
            (Value::F32(a), Value::Float(b)) | (Value::Float(b), Value::F32(a)) => {
                *a == AsPrimitive::<f32>::as_(*b)
            }
            (Value::Int(a), Value::Float(b)) | (Value::Float(b), Value::Int(a)) => {
                AsPrimitive::<f64>::as_(*a) == *b
            }
            (Value::Char(a), Value::Char(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            // Snapshots, not held guards. Comparing a value with its own clone sees the same
            // mutex on both sides, instant deadlock.
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
                // snapshots, see above
                let da = da.lock().clone();
                let db = db.lock().clone();
                EnumDef::same(ea, eb)
                    && va == vb
                    && da.len() == db.len()
                    && da.iter().zip(db.iter()).all(|(x, y)| x.eq_value(y))
            }
            // by content, insertion order doesn't matter. Snapshots, see above
            (Value::Map(a, ka), Value::Map(b, kb)) => {
                let a = a.lock().clone();
                let b = b.lock().clone();
                ka == kb
                    && a.len() == b.len()
                    && a.iter()
                        .all(|(key, value)| b.get(key).is_some_and(|other| value.eq_value(other)))
            }
            // Snapshot both field vectors first. Holding both guards and then calling `b.get`
            // deadlocks on every `PathBuf` comparison.
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
            // parse errors compare by kind, every other native by identity
            (Value::Native(a), Value::Native(b)) => {
                Arc::ptr_eq(a, b)
                    || matches!(
                        (&*a.lock(), &*b.lock()),
                        (
                            Native::ParseErr { debug: da, .. },
                            Native::ParseErr { debug: db, .. }
                        ) if da == db
                    )
            }
            _ => false,
        }
    }

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
            Value::Cell(kind, slot) if kind.is_shared_pointer() => {
                let inner = slot.lock().clone();
                inner.display()
            }
            // a reference displays what it points at
            Value::Ref(reference) => match reference.get() {
                Some(value) => value.display(),
                None => "<dangling reference>".to_string(),
            },
            Value::Native(n) => match &*n.lock() {
                Native::IoErr { display, .. }
                | Native::JoinErr { display, .. }
                | Native::ParseErr { display, .. } => display.clone(),
                other => format!("<{}>", other.type_name()),
            },
            // `VarError` implements `Display`, scripts print it with `{e}`
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

    pub fn debug(&self) -> String {
        super::debug_fmt::render(self, &super::debug_fmt::DebugOpts::plain())
    }
}

pub(super) fn big_text(v: i128, w: IntWidth) -> String {
    if w == IntWidth::U128 {
        v.cast_unsigned().to_string()
    } else {
        v.to_string()
    }
}

impl MapKey {
    pub fn to_value(&self) -> Value {
        match self {
            MapKey::Bool(b) => Value::Bool(*b),
            MapKey::Int(i) => Value::Int(*i),
            MapKey::Wide(v, w) => Value::IntW(*v, *w),
            MapKey::Char(c) => Value::Char(*c),
            MapKey::Str(s) => Value::Str(s.clone()),
            MapKey::Unit => Value::Unit,
            MapKey::Tuple(items) => Value::tuple(items.iter().map(MapKey::to_value).collect()),
            MapKey::Vec(items) => Value::vec(items.iter().map(MapKey::to_value).collect()),
            MapKey::Opt(None) => Value::none(),
            MapKey::Opt(Some(inner)) => Value::some(inner.to_value()),
            MapKey::Struct(shape, fields) => {
                Value::structure(shape.clone(), fields.iter().map(MapKey::to_value).collect())
            }
            MapKey::Enum(def, variant, payload) => Value::Enum {
                def: def.clone(),
                variant: *variant,
                data: Arc::new(Mutex::new(payload.iter().map(MapKey::to_value).collect())),
            },
        }
    }
}

/// Delegate to the host `Display` and `Debug`, that is exactly the target behavior.
fn format_float(f: f64) -> String {
    f.to_string()
}

pub(super) fn format_float_debug(f: f64) -> String {
    format!("{f:?}")
}
