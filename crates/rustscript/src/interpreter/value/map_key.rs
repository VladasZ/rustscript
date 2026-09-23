//! The hashable key form of a value. Manual `Hash` so the exact bytes per variant are fixed.

use std::cmp::Ordering;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::interpreter::enum_def::EnumDef;
use crate::interpreter::numeric::IntWidth;

use super::{RsStr, StructShape, Value};

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

/// The order of a derived `Ord`, which is what a `BTreeMap` key sorts by. A struct compares its
/// fields in declaration order and an enum its variant index first, then the payload.
/// `std::cmp::Reverse` flips its inner key.
impl Ord for MapKey {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (MapKey::Bool(a), MapKey::Bool(b)) => a.cmp(b),
            (MapKey::Int(_) | MapKey::Wide(..), MapKey::Int(_) | MapKey::Wide(..)) => {
                self.int_value().cmp(&other.int_value())
            }
            (MapKey::Char(a), MapKey::Char(b)) => a.cmp(b),
            (MapKey::Str(a), MapKey::Str(b)) => (**a).cmp(&**b),
            (MapKey::Unit, MapKey::Unit) => Ordering::Equal,
            (MapKey::Opt(a), MapKey::Opt(b)) => a.cmp(b),
            (MapKey::Struct(sa, a), MapKey::Struct(_, b)) if &*sa.name == "Reverse" => b.cmp(a),
            (MapKey::Tuple(a), MapKey::Tuple(b))
            | (MapKey::Vec(a), MapKey::Vec(b))
            | (MapKey::Struct(_, a), MapKey::Struct(_, b)) => a.cmp(b),
            (MapKey::Enum(_, va, a), MapKey::Enum(_, vb, b)) => va.cmp(vb).then_with(|| a.cmp(b)),
            // 1 real map never mixes key types, this only keeps the order total
            _ => self.rank().cmp(&other.rank()),
        }
    }
}

impl PartialOrd for MapKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl MapKey {
    fn int_value(&self) -> i128 {
        match self {
            MapKey::Int(i) => i128::from(*i),
            MapKey::Wide(v, w) => w.decode(*v),
            _ => 0,
        }
    }

    fn rank(&self) -> u8 {
        match self {
            MapKey::Bool(_) => 0,
            MapKey::Int(_) | MapKey::Wide(..) => 1,
            MapKey::Char(_) => 2,
            MapKey::Str(_) => 3,
            MapKey::Unit => 4,
            MapKey::Tuple(_) => 5,
            MapKey::Opt(_) => 6,
            MapKey::Vec(_) => 7,
            MapKey::Struct(..) => 8,
            MapKey::Enum(..) => 9,
        }
    }
}

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

pub(super) fn keys_of(values: &[Value]) -> Option<Vec<MapKey>> {
    values.iter().map(Value::as_key).collect()
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
