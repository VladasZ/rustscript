//! `serde_json` parsing and the coercion pass for annotated lets, serialization is in
//! `json_serialize`. Struct layouts are precomputed at load, so nothing here touches the syn AST,
//! which is not `Send`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::sync::Arc;

use anyhow::{Result, bail};
use rustc_hash::FxHashMap;

use super::bytecode::PathId;
use super::enum_def::{EnumKind, OK};
use super::json_typed::{OptVisitor, TypedVisitor};
use super::native::Native;
use super::numeric::IntWidth;
use super::serde_types::{DataError, EnumInfo};
use super::typeir::{ScalarIr, TypeIr};
use super::value::{MapKey, MapStore, RsStr, StructShape, Value};

pub(super) use super::json_serialize::{json_to_pvalue, pvalue_to_json};
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

impl StructInfo {
    /// The key the input names the field by, `rename` applied.
    fn key_of(&self, slot: usize) -> &str {
        self.key_map
            .iter()
            .find(|(_, s)| **s == slot)
            .map_or("?", |(k, _)| k.as_str())
    }
}

pub type Structs = HashMap<Arc<str>, Arc<StructInfo>>;

// coercion

impl Vm {
    pub(super) fn coerce_value(self: &Arc<Self>, value: Value, ty: &TypeIr) -> Result<Value> {
        Ok(match ty {
            TypeIr::Dynamic
            | TypeIr::Generic(_)
            | TypeIr::Scalar(_)
            | TypeIr::MapValue(_, false) => value,
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
            TypeIr::Vec(inner) => self.coerce_vec(value, inner)?,
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

    /// A vec coerces per element.
    fn coerce_vec(self: &Arc<Self>, value: Value, inner: &TypeIr) -> Result<Value> {
        let Value::Vec(items) = &value else {
            return Ok(value);
        };
        Ok(match inner {
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
            TypeIr::Dynamic
            | TypeIr::Generic(_)
            | TypeIr::Scalar(_)
            | TypeIr::MapValue(_, false) => value,
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
            let flatten = info.shape.serde.get(slot).is_some_and(|f| f.flatten);
            let raw = if flatten {
                // the keys no plain field takes, a flattened struct ignores the ones it lacks
                let mut rest = map.clone();
                rest.retain(|k, _| !matches!(k, MapKey::Str(s) if info.key_map.contains_key(&**s)));
                Value::map_of(rest)
            } else {
                match map.get(&MapKey::Str((&**key).into())) {
                    Some(v) => v.clone(),
                    None => match info.defaults.get(slot).and_then(Option::as_ref) {
                        Some(chunk) => self.run_chunk(chunk, &[], &[], false)?,
                        None => Value::none(),
                    },
                }
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
            TypeIr::Scalar(s) => JsonPlan::Scalar(*s),
            TypeIr::Generic(name) => match tenv.iter().find(|(n, _)| **n == **name) {
                Some((_, bound)) => self.json_plan(bound, building, tenv),
                None => JsonPlan::Dynamic,
            },
            TypeIr::Vec(inner) => JsonPlan::Vec(Box::new(self.json_plan(inner, building, tenv))),
            TypeIr::Set(inner, sorted) => {
                JsonPlan::Set(Box::new(self.json_plan(inner, building, tenv)), *sorted)
            }
            TypeIr::Option(inner) => JsonPlan::Opt(Box::new(self.json_plan(inner, building, tenv))),
            TypeIr::MapValue(inner, sorted) => {
                JsonPlan::Map(Box::new(self.json_plan(inner, building, tenv)), *sorted)
            }
            TypeIr::Enum(canon) => match self.serde_enums.get(&**canon) {
                Some(info) => JsonPlan::Enum(info.clone()),
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

    /// `serde_json::from_str::<T>`, `toml::from_str` or `serde_yaml::from_str` with a known
    /// target type, `format` is the path that names which
    pub(super) fn typed_from_str(
        self: &Arc<Self>,
        format: PathId,
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
        let parsed = match format {
            PathId::TomlFromStr => toml::Deserializer::parse(text)
                .and_then(|de| parse_planned(de, &plan, self))
                .map_err(|e| parse_error(&e)),
            PathId::SerdeYamlFromStr => {
                parse_planned(serde_yaml::Deserializer::from_str(text), &plan, self)
                    .map_err(|e| parse_error(&e))
            }
            _ => parse_json_planned(text, &plan, self).map_err(|e| parse_error(&e)),
        };
        Ok(match parsed {
            Ok(v) => Value::ok(v),
            Err(e) => Value::err(e),
        })
    }
}

// parsing

/// The error of a parse as a value. `{}` and `{:?}` print what the real error prints,
/// `Error("missing field `x`", line: 1, column: 8)` for `serde_json`.
pub(super) fn parse_error(error: &(impl Display + Debug)) -> Value {
    Native::ParseErr {
        display: error.to_string(),
        debug: format!("{error:?}"),
    }
    .wrap()
}

pub(super) enum JsonPlan {
    Dynamic,
    Vec(Box<JsonPlan>),
    /// a json array packed into a set, the flag is a `BTreeSet`
    Set(Box<JsonPlan>, bool),
    /// the flag is a `BTreeMap`
    Map(Box<JsonPlan>, bool),
    Struct(Arc<StructPlan>),
    /// read as a plain json tree first, see `serde_types`
    Enum(Arc<EnumInfo>),
    /// read with the real serde visitor of the primitive, see `scalar_seed`
    Scalar(ScalarIr),
    /// `Option<T>`, a null is `None` before the inner plan sees it, see `OptVisitor`
    Opt(Box<JsonPlan>),
}

pub(super) struct StructPlan {
    info: Arc<StructInfo>,
    fields: Vec<JsonPlan>,
}

impl StructPlan {
    pub(super) fn info(&self) -> &Arc<StructInfo> {
        &self.info
    }

    pub(super) fn fields(&self) -> &[JsonPlan] {
        &self.fields
    }

    /// What the field holds for a value its plan read. An `Opt` plan already made the `Some`. An
    /// `Option` field whose inner type has no plan wraps here.
    pub(super) fn field_value(&self, slot: usize, value: Value) -> Value {
        let planned = matches!(self.fields[slot], JsonPlan::Opt(_));
        if self.info.optional[slot] && !planned && !value.is_none_value() {
            Value::some(value)
        } else {
            value
        }
    }
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
    let mut de = serde_json::Deserializer::from_str(text);
    let v = parse_planned(&mut de, plan, vm)?;
    de.end()?;
    Ok(v)
}

/// One typed parse over any serde format, so a toml or yaml input reports what its own crate
/// reports, a missing field included.
fn parse_planned<'de, D: serde::Deserializer<'de>>(
    de: D,
    plan: &JsonPlan,
    vm: &Arc<Vm>,
) -> std::result::Result<Value, D::Error> {
    use serde::de::DeserializeSeed;
    let cx = ParseCx {
        keys: RefCell::new(FxHashMap::default()),
        vm: Some(vm),
    };
    PlanSeed { plan, cx: &cx }.deserialize(de)
}

/// What every level of 1 parse shares. The vm runs `#[serde(default)]` functions, a parse
/// without one treats a defaulted field as missing.
pub(super) struct ParseCx<'v> {
    keys: JsonKeys,
    pub(super) vm: Option<&'v Arc<Vm>>,
}

pub(super) struct PlanSeed<'a> {
    pub(super) plan: &'a JsonPlan,
    pub(super) cx: &'a ParseCx<'a>,
}

impl<'de> serde::de::DeserializeSeed<'de> for PlanSeed<'_> {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        d: D,
    ) -> std::result::Result<Value, D::Error> {
        // serde buffers an untagged enum and tries the variants after the value is read, so its
        // error carries no position of its own
        if let JsonPlan::Opt(inner) = self.plan {
            return d.deserialize_option(OptVisitor { inner, cx: self.cx });
        }
        if let JsonPlan::Enum(info) = self.plan
            && matches!(info.def.serde.repr, super::enum_def::SerdeRepr::Untagged)
        {
            let raw = d.deserialize_any(PlanVisitor {
                plan: &JsonPlan::Dynamic,
                cx: self.cx,
            })?;
            return match self.cx.vm {
                Some(vm) => vm
                    .enum_from_json(info, raw)
                    .map_err(serde::de::Error::custom),
                None => Ok(raw),
            };
        }
        let typed = TypedVisitor {
            plan: self.plan,
            cx: self.cx,
        };
        match self.plan {
            JsonPlan::Scalar(scalar) => super::json_scalar::scalar_seed(d, *scalar),
            JsonPlan::Enum(info) => {
                d.deserialize_any(super::serde_types::EnumVisitor { info, cx: self.cx })
            }
            // a tuple struct has no named field and keeps the loose read
            JsonPlan::Struct(sp) if !sp.info.shape.fields.is_empty() => {
                d.deserialize_struct("", &[], typed)
            }
            JsonPlan::Vec(_) | JsonPlan::Set(..) => d.deserialize_seq(typed),
            JsonPlan::Map(..) => d.deserialize_map(typed),
            _ => d.deserialize_any(PlanVisitor {
                plan: self.plan,
                cx: self.cx,
            }),
        }
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

/// Resolves an object key to its slot without allocating. An unknown key is kept only when a
/// `flatten` field needs it.
struct FieldSeed<'a> {
    key_map: &'a FxHashMap<String, usize>,
    keep_unknown: bool,
}

enum FieldKey {
    Slot(usize),
    Unknown(String),
    Skip,
}

impl<'de> serde::de::DeserializeSeed<'de> for FieldSeed<'_> {
    type Value = FieldKey;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        d: D,
    ) -> std::result::Result<FieldKey, D::Error> {
        d.deserialize_str(self)
    }
}

impl serde::de::Visitor<'_> for FieldSeed<'_> {
    type Value = FieldKey;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an object key")
    }

    fn visit_str<E: serde::de::Error>(self, s: &str) -> std::result::Result<FieldKey, E> {
        Ok(match self.key_map.get(s) {
            Some(slot) => FieldKey::Slot(*slot),
            None if self.keep_unknown => FieldKey::Unknown(s.to_string()),
            None => FieldKey::Skip,
        })
    }
}

/// Fills each `flatten` field from the keys no other field took, in field order. A flattened
/// struct takes its own keys out, so a later flattened map sees only the rest, like serde.
fn fill_flattened<E: serde::de::Error>(
    sp: &StructPlan,
    values: &mut [Value],
    filled: &mut [bool],
    mut rest: serde_json::Map<String, serde_json::Value>,
    cx: &ParseCx<'_>,
) -> std::result::Result<(), E> {
    use serde::de::DeserializeSeed;
    for (slot, attrs) in sp.info.shape.serde.iter().enumerate() {
        if !attrs.flatten {
            continue;
        }
        // an `Option` around a flattened field reads the inner type from the same keys
        let (plan, optional) = match &sp.fields[slot] {
            JsonPlan::Opt(inner) => (&**inner, true),
            plan => (plan, sp.info.optional[slot]),
        };
        let v = PlanSeed { plan, cx }
            .deserialize(serde_json::Value::Object(rest.clone()))
            .map_err(E::custom)?;
        if let JsonPlan::Struct(inner) = plan {
            rest.retain(|k, _| !inner.info.key_map.contains_key(k));
        }
        values[slot] = if optional && !v.is_none_value() {
            Value::some(v)
        } else {
            v
        };
        filled[slot] = true;
    }
    Ok(())
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
                let keep_unknown = sp.info.shape.serde.iter().any(|f| f.flatten);
                let mut rest = serde_json::Map::new();
                while let Some(slot) = access.next_key_seed(FieldSeed {
                    key_map: &sp.info.key_map,
                    keep_unknown,
                })? {
                    match slot {
                        FieldKey::Unknown(key) => {
                            rest.insert(key, access.next_value()?);
                        }
                        FieldKey::Slot(i) => {
                            // a derived struct refuses a key it already read, before its value
                            if filled[i] {
                                let key = sp.info.key_of(i);
                                return Err(serde::de::Error::custom(format!(
                                    "duplicate field `{key}`"
                                )));
                            }
                            let v = access.next_value_seed(PlanSeed {
                                plan: &sp.fields[i],
                                cx: self.cx,
                            })?;
                            values[i] = sp.field_value(i, v);
                            filled[i] = true;
                        }
                        FieldKey::Skip => {
                            access.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                if keep_unknown {
                    fill_flattened(sp, &mut values, &mut filled, rest, self.cx)?;
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
        let key = info.key_of(slot);
        return Err(E::custom(format!("missing field `{key}`")));
    }
    Ok(())
}
