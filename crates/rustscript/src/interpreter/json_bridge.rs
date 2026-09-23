//! `serde_json` parsing, serialization and the coercion pass for annotated lets. Struct layouts are
//! precomputed at load, so nothing here touches the syn AST, which is not `Send`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, bail};
use rustc_hash::FxHashMap;

use super::bridge::arg;
use super::bytecode::PathId;
use super::enum_def::{EnumKind, OK, SOME};
use super::numeric::IntWidth;
use super::serde_types::{DataError, EnumInfo, enum_to_json};
use super::typeir::TypeIr;
use super::value::{MapKey, MapStore, RsStr, StructShape, Value};
use super::vm::Vm;

/// precomputed at load
pub struct StructInfo {
    pub shape: Arc<StructShape>,
    pub coerce: Vec<Option<TypeIr>>,
    pub json: Vec<TypeIr>,
    pub optional: Vec<bool>,
    /// `#[serde(rename)]` applied
    pub key_map: FxHashMap<String, usize>,
    /// per field, the `#[serde(default)]` chunk that makes a missing value
    pub defaults: Vec<Option<Arc<super::bytecode::Chunk>>>,
}

pub type Structs = HashMap<Arc<str>, Arc<StructInfo>>;

fn is_none(value: &Value) -> bool {
    matches!(value, Value::Enum { def, variant, .. } if def.kind == EnumKind::Option && *variant != SOME)
}

// coercion

impl Vm {
    pub(super) fn coerce_value(self: &Arc<Self>, value: Value, ty: &TypeIr) -> Result<Value> {
        Ok(match ty {
            TypeIr::Dynamic | TypeIr::Generic(_) | TypeIr::MapValue(_, false) => value,
            TypeIr::MapValue(_, true) => {
                let Value::Map(m, kind) = &value else {
                    return Ok(value);
                };
                if m.lock().is_sorted() {
                    return Ok(value);
                }
                let mut sorted = super::value::MapStore::sorted();
                sorted.extend(m.lock().iter().map(|(k, v)| (k.clone(), v.clone())));
                Value::Map(Arc::new(parking_lot::Mutex::new(sorted)), *kind)
            }
            TypeIr::Vec(inner) => {
                let Value::Vec(items) = &value else {
                    return Ok(value);
                };
                match &**inner {
                    // a struct element type resolves once for the whole vector
                    TypeIr::Struct(canon) => match self.structs.get(&**canon) {
                        Some(info) => {
                            let items = items.lock().clone();
                            let mut out = Vec::with_capacity(items.len());
                            for v in items {
                                out.push(match &v {
                                    Value::Map(m, _) => {
                                        let map = m.lock().clone();
                                        self.struct_from_map(info, &map)?
                                    }
                                    _ => v,
                                });
                            }
                            Value::vec(out)
                        }
                        None => value,
                    },
                    TypeIr::Vec(_)
                    | TypeIr::Option(_)
                    | TypeIr::Set(..)
                    | TypeIr::Enum(_)
                    | TypeIr::MapValue(_, true) => {
                        let items = items.lock().clone();
                        let mut out = Vec::with_capacity(items.len());
                        for v in items {
                            out.push(self.coerce_value(v, inner)?);
                        }
                        Value::vec(out)
                    }
                    TypeIr::Dynamic | TypeIr::Generic(_) | TypeIr::MapValue(_, false) => value,
                }
            }
            TypeIr::Set(inner, sorted) => {
                // a `collect()` lands here as a Vec and packs into the shared map storage
                if let Value::Map(m, _) = &value
                    && m.lock().is_sorted() == *sorted
                {
                    return Ok(Value::Map(m.clone(), super::value::MapKind::Set));
                }
                let items: Vec<Value> = match &value {
                    Value::Vec(items) => items.lock().clone(),
                    Value::Map(m, _) => m.lock().keys().map(MapKey::to_value).collect(),
                    _ => return Ok(value),
                };
                let mut set = if *sorted {
                    super::value::MapStore::sorted()
                } else {
                    super::value::MapStore::default()
                };
                for v in &items {
                    // an element that can't be a key leaves the value alone
                    let Some(key) = self.coerce_value(v.clone(), inner)?.into_key() else {
                        return Ok(value.clone());
                    };
                    set.insert(key, Value::Unit);
                }
                Value::set_of(set)
            }
            TypeIr::Option(inner) => {
                if let Some(payload) = value.some_payload() {
                    return Ok(Value::some(self.coerce_value(payload, inner)?));
                }
                value
            }
            TypeIr::Struct(canon) => {
                if let Value::Map(map, _) = &value
                    && let Some(info) = self.structs.get(&**canon)
                {
                    let map = map.lock().clone();
                    return self.struct_from_map(info, &map);
                }
                value
            }
            // a value built by the script is already the enum, a parsed one reads from json
            TypeIr::Enum(canon) => match (&value, self.serde_enums.get(&**canon)) {
                (Value::Enum { .. } | Value::Struct(_), _) | (_, None) => value,
                (_, Some(info)) => {
                    let info = info.clone();
                    self.enum_from_json(&info, value)
                        .map_err(|e| anyhow::Error::new(DataError(e)))?
                }
            },
        })
    }

    pub(super) fn coerce_result(self: &Arc<Self>, value: Value, ty: &TypeIr) -> Result<Value> {
        if let Value::Enum { def, variant, data } = &value
            && def.kind == EnumKind::Result
            && *variant == OK
        {
            let inner = data.lock().first().cloned();
            if let Some(inner) = inner {
                // json that does not fit the type is the parse's `Err`, like serde
                return match self.coerce_value(inner, ty) {
                    Ok(v) => Ok(Value::ok(v)),
                    Err(e) if e.is::<DataError>() => Ok(Value::err(Value::str(e.to_string()))),
                    Err(e) => Err(e),
                };
            }
        }
        self.coerce_value(value, ty)
    }

    /// A missing key with `#[serde(default)]` runs its default, any other missing key is None.
    fn struct_from_map(self: &Arc<Self>, info: &StructInfo, map: &MapStore) -> Result<Value> {
        let mut values = Vec::with_capacity(info.coerce.len());
        for (slot, (fname, ty)) in info.shape.fields.iter().zip(&info.coerce).enumerate() {
            // the input names a field by its serde name, `rename` and `rename_all` applied
            let key = info
                .shape
                .renames
                .get(slot)
                .and_then(Option::as_ref)
                .unwrap_or(fname);
            let raw = match map.get(&MapKey::Str((&**key).into())) {
                Some(v) => v.clone(),
                None => match info.defaults.get(slot).and_then(Option::as_ref) {
                    Some(chunk) => self.run_chunk(chunk, &[], &[], false)?,
                    None => Value::none(),
                },
            };
            let coerced = match ty {
                Some(t) => self.coerce_value(raw, t)?,
                None => raw,
            };
            values.push(coerced);
        }
        Ok(Value::structure(info.shape.clone(), values))
    }

    /// `building` guards recursive structs
    pub(super) fn json_plan(
        &self,
        ty: &TypeIr,
        building: &mut Vec<String>,
        tenv: &[(Arc<str>, TypeIr)],
    ) -> JsonPlan {
        match ty {
            TypeIr::Dynamic => JsonPlan::Dynamic,
            TypeIr::Generic(name) => match tenv.iter().find(|(n, _)| **n == **name) {
                Some((_, bound)) => self.json_plan(bound, building, tenv),
                None => JsonPlan::Dynamic,
            },
            TypeIr::Vec(inner) => JsonPlan::Vec(Box::new(self.json_plan(inner, building, tenv))),
            TypeIr::Set(inner, sorted) => {
                JsonPlan::Set(Box::new(self.json_plan(inner, building, tenv)), *sorted)
            }
            TypeIr::Option(inner) => match self.json_plan(inner, building, tenv) {
                JsonPlan::Enum(info, _) => JsonPlan::Enum(info, true),
                plan => plan,
            },
            TypeIr::MapValue(inner, sorted) => {
                JsonPlan::Map(Box::new(self.json_plan(inner, building, tenv)), *sorted)
            }
            TypeIr::Enum(canon) => match self.serde_enums.get(&**canon) {
                Some(info) => JsonPlan::Enum(info.clone(), false),
                None => JsonPlan::Dynamic,
            },
            TypeIr::Struct(canon) => {
                if building.iter().any(|b| b.as_str() == &**canon) {
                    return JsonPlan::Dynamic;
                }
                let Some(info) = self.structs.get(&**canon) else {
                    return JsonPlan::Dynamic;
                };
                building.push(canon.to_string());
                let fields = info
                    .json
                    .iter()
                    .map(|fir| self.json_plan(fir, building, &[]))
                    .collect();
                building.pop();
                JsonPlan::Struct(Arc::new(StructPlan {
                    info: info.clone(),
                    fields,
                }))
            }
        }
    }

    /// `serde_json::from_str::<T>` with a known target type
    pub(super) fn typed_from_str(
        self: &Arc<Self>,
        args: &[Value],
        ty: &TypeIr,
        tenv: &[(Arc<str>, TypeIr)],
    ) -> Result<Value> {
        let owned;
        let text: &str = match args.first() {
            Some(Value::Str(s)) => s,
            Some(other) => {
                owned = other.display();
                &owned
            }
            None => bail!("from_str needs a string"),
        };
        let plan = self.json_plan(ty, &mut Vec::new(), tenv);
        Ok(match parse_json_planned(text, &plan, self) {
            Ok(v) => Value::ok(v),
            Err(e) => Value::err(Value::str(e.to_string())),
        })
    }
}

// parsing

pub(super) enum JsonPlan {
    Dynamic,
    Vec(Box<JsonPlan>),
    /// a json array packed into a set, the flag is a `BTreeSet`
    Set(Box<JsonPlan>, bool),
    /// the flag is a `BTreeMap`
    Map(Box<JsonPlan>, bool),
    Struct(Arc<StructPlan>),
    /// read as a plain json tree first, see `serde_types`. The flag is an `Option` around it,
    /// where a json null is `None` before any variant sees it.
    Enum(Arc<EnumInfo>, bool),
}

pub(super) struct StructPlan {
    info: Arc<StructInfo>,
    fields: Vec<JsonPlan>,
}

/// Object keys repeat for every array element, so each parse interns them. The parse runs on 1
/// thread, so a `RefCell` is fine.
type JsonKeys = RefCell<FxHashMap<String, RsStr>>;

pub(super) fn parse_json(text: &str) -> std::result::Result<Value, serde_json::Error> {
    use serde::de::DeserializeSeed;
    let mut de = serde_json::Deserializer::from_str(text);
    let cx = ParseCx {
        keys: RefCell::new(FxHashMap::default()),
        vm: None,
    };
    let v = PlanSeed {
        plan: &JsonPlan::Dynamic,
        cx: &cx,
    }
    .deserialize(&mut de)?;
    de.end()?;
    Ok(v)
}

fn parse_json_planned(
    text: &str,
    plan: &JsonPlan,
    vm: &Arc<Vm>,
) -> std::result::Result<Value, serde_json::Error> {
    use serde::de::DeserializeSeed;
    let mut de = serde_json::Deserializer::from_str(text);
    let cx = ParseCx {
        keys: RefCell::new(FxHashMap::default()),
        vm: Some(vm),
    };
    let v = PlanSeed { plan, cx: &cx }.deserialize(&mut de)?;
    de.end()?;
    Ok(v)
}

/// What every level of 1 parse shares. The vm runs `#[serde(default)]` functions, a parse
/// without one treats a defaulted field as missing.
pub(super) struct ParseCx<'v> {
    keys: JsonKeys,
    pub(super) vm: Option<&'v Arc<Vm>>,
}

struct PlanSeed<'a> {
    plan: &'a JsonPlan,
    cx: &'a ParseCx<'a>,
}

impl<'de> serde::de::DeserializeSeed<'de> for PlanSeed<'_> {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        d: D,
    ) -> std::result::Result<Value, D::Error> {
        // serde buffers an untagged enum and tries the variants after the value is read, so its
        // error carries no position of its own
        if let JsonPlan::Enum(info, optional) = self.plan
            && matches!(info.def.serde.repr, super::enum_def::SerdeRepr::Untagged)
        {
            let raw = d.deserialize_any(PlanVisitor {
                plan: &JsonPlan::Dynamic,
                cx: self.cx,
            })?;
            return match self.cx.vm {
                Some(_) if *optional && raw.is_none_value() => Ok(raw),
                Some(vm) => vm
                    .enum_from_json(info, raw)
                    .map_err(serde::de::Error::custom),
                None => Ok(raw),
            };
        }
        if let JsonPlan::Enum(info, optional) = self.plan {
            return d.deserialize_any(super::serde_types::EnumVisitor {
                info,
                cx: self.cx,
                optional: *optional,
            });
        }
        d.deserialize_any(PlanVisitor {
            plan: self.plan,
            cx: self.cx,
        })
    }
}

struct KeySeed<'a> {
    keys: &'a JsonKeys,
}

impl KeySeed<'_> {
    fn intern(&self, s: &str) -> RsStr {
        if let Some(k) = self.keys.borrow().get(s) {
            return k.clone();
        }
        let k = RsStr::from(s);
        self.keys.borrow_mut().insert(s.to_string(), k.clone());
        k
    }
}

impl<'de> serde::de::DeserializeSeed<'de> for KeySeed<'_> {
    type Value = RsStr;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        d: D,
    ) -> std::result::Result<RsStr, D::Error> {
        d.deserialize_str(self)
    }
}

impl serde::de::Visitor<'_> for KeySeed<'_> {
    type Value = RsStr;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an object key")
    }

    fn visit_str<E: serde::de::Error>(self, s: &str) -> std::result::Result<RsStr, E> {
        Ok(self.intern(s))
    }

    fn visit_string<E: serde::de::Error>(self, s: String) -> std::result::Result<RsStr, E> {
        Ok(self.intern(&s))
    }
}

/// Resolves an object key to its slot without allocating. Unknown keys are skipped.
struct FieldSeed<'a> {
    key_map: &'a FxHashMap<String, usize>,
}

impl<'de> serde::de::DeserializeSeed<'de> for FieldSeed<'_> {
    type Value = Option<usize>;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        d: D,
    ) -> std::result::Result<Option<usize>, D::Error> {
        d.deserialize_str(self)
    }
}

impl serde::de::Visitor<'_> for FieldSeed<'_> {
    type Value = Option<usize>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an object key")
    }

    fn visit_str<E: serde::de::Error>(self, s: &str) -> std::result::Result<Option<usize>, E> {
        Ok(self.key_map.get(s).copied())
    }
}

pub(super) struct PlanVisitor<'a> {
    pub(super) plan: &'a JsonPlan,
    pub(super) cx: &'a ParseCx<'a>,
}

impl<'de> serde::de::Visitor<'de> for PlanVisitor<'_> {
    type Value = Value;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a json value")
    }

    fn visit_bool<E>(self, b: bool) -> std::result::Result<Value, E> {
        Ok(Value::Bool(b))
    }

    fn visit_i64<E>(self, i: i64) -> std::result::Result<Value, E> {
        Ok(Value::Int(i))
    }

    fn visit_u64<E>(self, u: u64) -> std::result::Result<Value, E> {
        // a u64 past `i64::MAX` keeps its width instead of becoming a float
        Ok(match i64::try_from(u) {
            Ok(i) => Value::Int(i),
            Err(_) => Value::int_of_width(i128::from(u), IntWidth::U64),
        })
    }

    fn visit_f64<E>(self, f: f64) -> std::result::Result<Value, E> {
        Ok(Value::Float(f))
    }

    fn visit_str<E>(self, s: &str) -> std::result::Result<Value, E> {
        Ok(Value::str(s))
    }

    fn visit_string<E>(self, s: String) -> std::result::Result<Value, E> {
        Ok(Value::str(s))
    }

    fn visit_unit<E>(self) -> std::result::Result<Value, E> {
        Ok(Value::none())
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(
        self,
        mut seq: A,
    ) -> std::result::Result<Value, A::Error> {
        let (elem, set) = match self.plan {
            JsonPlan::Vec(p) => (&**p, None),
            JsonPlan::Set(p, sorted) => (&**p, Some(*sorted)),
            _ => (&JsonPlan::Dynamic, None),
        };
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(v) = seq.next_element_seed(PlanSeed {
            plan: elem,
            cx: self.cx,
        })? {
            items.push(v);
        }
        match set {
            Some(sorted) => super::vecmap::collect_set(items, sorted)
                .map_err(|e| serde::de::Error::custom(e.to_string())),
            None => Ok(Value::vec(items)),
        }
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(
        self,
        mut access: A,
    ) -> std::result::Result<Value, A::Error> {
        match self.plan {
            JsonPlan::Struct(sp) => {
                let mut values: Vec<Value> = (0..sp.info.shape.fields.len())
                    .map(|_| Value::none())
                    .collect();
                let mut filled = vec![false; values.len()];
                while let Some(slot) = access.next_key_seed(FieldSeed {
                    key_map: &sp.info.key_map,
                })? {
                    match slot {
                        Some(i) => {
                            let v = access.next_value_seed(PlanSeed {
                                plan: &sp.fields[i],
                                cx: self.cx,
                            })?;
                            // an Option field wraps a present value in Some
                            values[i] = if sp.info.optional[i] && !v.is_none_value() {
                                Value::some(v)
                            } else {
                                v
                            };
                            filled[i] = true;
                        }
                        None => {
                            access.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                fill_missing(&mut values, &filled, &sp.info, self.cx.vm)?;
                Ok(Value::structure(sp.info.shape.clone(), values))
            }
            plan => {
                let (elem, sorted) = match plan {
                    JsonPlan::Map(p, sorted) => (&**p, *sorted),
                    _ => (&JsonPlan::Dynamic, false),
                };
                let mut map = if sorted {
                    super::value::MapStore::sorted()
                } else {
                    super::value::MapStore::default()
                };
                while let Some(k) = access.next_key_seed(KeySeed {
                    keys: &self.cx.keys,
                })? {
                    map.insert(
                        MapKey::Str(k),
                        access.next_value_seed(PlanSeed {
                            plan: elem,
                            cx: self.cx,
                        })?,
                    );
                }
                Ok(Value::map_of(map))
            }
        }
    }
}

// serialization

/// For the toml and yaml bridges. Null maps to None like the json parser.
pub(super) fn json_to_pvalue(v: serde_json::Value) -> Value {
    use serde_json::Value as JsonValue;
    match v {
        JsonValue::Null => Value::none(),
        JsonValue::Bool(b) => Value::Bool(b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(u) = n.as_u64() {
                Value::int_of_width(i128::from(u), super::numeric::IntWidth::U64)
            } else {
                Value::Float(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        JsonValue::String(s) => Value::str(s),
        JsonValue::Array(items) => Value::vec(items.into_iter().map(json_to_pvalue).collect()),
        JsonValue::Object(map) => {
            let mut out = super::value::MapStore::default();
            for (k, v) in map {
                if let Some(key) = Value::str(k).into_key() {
                    out.insert(key, json_to_pvalue(v));
                }
            }
            Value::map_of(out)
        }
    }
}

pub(super) fn pvalue_to_json(v: &Value) -> Result<serde_json::Value> {
    use serde_json::Value as JsonValue;
    Ok(match v {
        Value::Unit => JsonValue::Null,
        Value::Bool(b) => JsonValue::Bool(*b),
        Value::Int(i) => JsonValue::Number(serde_json::Number::from(*i)),
        Value::IntW(bits, w) => {
            let value = w.decode(*bits);
            match i64::try_from(value) {
                Ok(small) => JsonValue::Number(serde_json::Number::from(small)),
                Err(_) => JsonValue::Number(serde_json::Number::from(
                    u64::try_from(value).expect("width-tagged value fits u64"),
                )),
            }
        }
        // a 128 bit integer is a number only while it fits the json range
        Value::Big(raw, w) => {
            let as_i64 = if *w == super::numeric::IntWidth::U128 {
                i64::try_from(raw.cast_unsigned()).map_err(|_| ())
            } else {
                i64::try_from(*raw).map_err(|_| ())
            };
            match as_i64 {
                Ok(small) => JsonValue::Number(serde_json::Number::from(small)),
                Err(()) => bail!("128-bit integer does not fit a json number"),
            }
        }
        Value::Float(f) => {
            serde_json::Number::from_f64(*f).map_or(JsonValue::Null, JsonValue::Number)
        }
        Value::F32(f) => {
            serde_json::Number::from_f64(f64::from(*f)).map_or(JsonValue::Null, JsonValue::Number)
        }
        Value::Char(c) => JsonValue::String(c.to_string()),
        Value::Str(s) => JsonValue::String(s.to_string()),
        Value::Vec(items) | Value::Tuple(items) => JsonValue::Array(
            items
                .lock()
                .iter()
                .map(pvalue_to_json)
                .collect::<Result<_>>()?,
        ),
        // serde writes a set as a list
        Value::Map(map, super::value::MapKind::Set) => JsonValue::Array(
            map.lock()
                .keys()
                .map(|k| pvalue_to_json(&k.to_value()))
                .collect::<Result<_>>()?,
        ),
        Value::Map(map, _) => {
            let mut obj = serde_json::Map::default();
            for (k, val) in map.lock().iter() {
                obj.insert(k.to_value().display(), pvalue_to_json(val)?);
            }
            JsonValue::Object(obj)
        }
        Value::Struct(s) => struct_to_json(s)?,
        Value::Enum { def, variant, data } if def.user => {
            super::serde_types::user_enum_to_json(def, *variant, &data.lock())?
        }
        Value::Enum { def, variant, data } => {
            let payload = data.lock().clone();
            if def.kind == EnumKind::Option {
                if *variant == SOME {
                    pvalue_to_json(&payload[0])?
                } else {
                    JsonValue::Null
                }
            } else if payload.is_empty() {
                JsonValue::String(def.variant_name(*variant).to_string())
            } else {
                let mut obj = serde_json::Map::default();
                obj.insert(
                    def.variant_name(*variant).to_string(),
                    JsonValue::Array(payload.iter().map(pvalue_to_json).collect::<Result<_>>()?),
                );
                JsonValue::Object(obj)
            }
        }
        Value::Range { .. } => bail!("cannot serialize a range to json"),
        Value::Closure(_) => bail!("cannot serialize a closure to json"),
        // serde serializes cells by content
        Value::Cell(_, slot) => {
            let inner = slot.lock().clone();
            pvalue_to_json(&inner)?
        }
        Value::Ref(reference) => {
            let Some(value) = reference.get() else {
                bail!("cannot serialize a dangling reference to json");
            };
            pvalue_to_json(&value)?
        }
        Value::Native(n) => bail!("cannot serialize a {} to json", n.lock().type_name()),
    })
}

/// A struct as a json object, `rename` and `skip_serializing_if` applied. A struct variant
/// goes inside its enum representation.
fn struct_to_json(s: &super::value::StructData) -> Result<serde_json::Value> {
    use serde_json::Value as JsonValue;
    let mut obj = serde_json::Map::default();
    let values = s.values.lock();
    for (slot, (field, val)) in s.shape.fields.iter().zip(values.iter()).enumerate() {
        if s.shape.skip_none.get(slot).copied().unwrap_or(false) && is_none(val) {
            continue;
        }
        let key = s
            .shape
            .renames
            .get(slot)
            .and_then(Option::as_ref)
            .unwrap_or(field);
        obj.insert(key.to_string(), pvalue_to_json(val)?);
    }
    Ok(match &s.shape.variant {
        Some((def, index)) => enum_to_json(def, *index, Some(JsonValue::Object(obj)))?,
        None => JsonValue::Object(obj),
    })
}

/// The dynamic path, `from_str` with no type plus `to_string` and `to_string_pretty`.
pub(super) fn bridge_serde_json(id: PathId, args: &[Value]) -> Result<Value> {
    match id {
        PathId::SerdeJsonFromStr => {
            let owned;
            let s: &str = match args.first() {
                Some(Value::Str(s)) => s,
                Some(other) => {
                    owned = other.display();
                    &owned
                }
                None => bail!("from_str needs a string"),
            };
            match parse_json(s) {
                Ok(v) => Ok(Value::ok(v)),
                Err(e) => Ok(Value::err(Value::str(e.to_string()))),
            }
        }
        PathId::SerdeJsonToString | PathId::SerdeJsonToStringPretty => {
            let v = arg(args, 0)?;
            let j = pvalue_to_json(&v)?;
            let s = if id == PathId::SerdeJsonToStringPretty {
                serde_json::to_string_pretty(&j)?
            } else {
                serde_json::to_string(&j)?
            };
            Ok(Value::ok(Value::str(s)))
        }
        PathId::SerdeJsonToValue => {
            let v = arg(args, 0)?;
            Ok(Value::ok(json_to_pvalue(pvalue_to_json(&v)?)))
        }
        _ => bail!("unsupported serde_json function `{id}`"),
    }
}

/// The fields the input left out, in declaration order like serde's derive. A
/// `#[serde(default)]` field runs its default, an `Option` stays None, and the first other one
/// fails the parse, after the defaults before it ran.
fn fill_missing<E: serde::de::Error>(
    values: &mut [Value],
    filled: &[bool],
    info: &StructInfo,
    vm: Option<&Arc<Vm>>,
) -> std::result::Result<(), E> {
    for (slot, done) in filled.iter().enumerate() {
        if *done {
            continue;
        }
        if let (Some(chunk), Some(vm)) = (info.defaults.get(slot).and_then(Option::as_ref), vm) {
            values[slot] = vm
                .run_chunk(chunk, &[], &[], false)
                .map_err(|e| E::custom(format!("{e:#}")))?;
            continue;
        }
        if info.optional.get(slot).copied().unwrap_or(false) {
            continue;
        }
        let key = info
            .key_map
            .iter()
            .find(|(_, s)| **s == slot)
            .map_or("?", |(k, _)| k.as_str());
        return Err(E::custom(format!("missing field `{key}`")));
    }
    Ok(())
}
