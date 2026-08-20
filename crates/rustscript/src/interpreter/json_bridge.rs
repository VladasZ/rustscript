//! `serde_json`: dynamic and typed parsing straight
//! into `Value`, serialization back to json text, and the coercion pass for
//! annotated lets. The `Value` twin of `json_bridge.rs` and the coercion
//! half of `eval.rs`.
//!
//! Struct layouts come from a table precomputed at load on the main thread,
//! so nothing here touches the resolver or the syn AST, which are not `Send`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, bail};
use rustc_hash::FxHashMap;

use super::Interp;
use super::bytecode::PathId as P;
use super::enum_def::{EnumKind, OK, SOME};
use super::numeric::IntWidth;
use super::typeir::{TypeIr, lower_type};
use super::value::{MapKey, RsStr, StructShape, Value};
use super::vm::Vm;

/// Everything the interpreter needs to know about one user struct,
/// precomputed at load: the runtime layout, the lowered field types for
/// coercion and typed json, and the json key mapping with serde renames.
pub struct StructInfo {
    pub shape: Arc<StructShape>,
    /// Per field, its lowered type when coercion can change the value.
    pub coerce: Vec<Option<TypeIr>>,
    /// Per field, its lowered type for json planning.
    pub json: Vec<TypeIr>,
    /// Whether field i was declared `Option<T>` in the source.
    pub optional: Vec<bool>,
    /// Json object key to field slot, `#[serde(rename)]` applied.
    pub key_map: FxHashMap<String, usize>,
}

pub type Structs = HashMap<Arc<str>, Arc<StructInfo>>;

impl Interp {
    /// Build the struct table from the AST, once at load on the main thread.
    /// Mirrors what `struct_shape` and `json_plan` read lazily from the AST.
    pub(super) fn build_structs(&self) -> Structs {
        let mut out = Structs::default();
        for (canon, def) in self.structs() {
            let module = def.module;
            let ast = def.ast.clone();
            let mut fields: Vec<Arc<str>> = Vec::new();
            let mut renames: Vec<Option<Arc<str>>> = Vec::new();
            let mut coerce = Vec::new();
            let mut json = Vec::new();
            let mut optional = Vec::new();
            let mut key_map = FxHashMap::default();
            let rule = super::serde_attrs::serde_rename_all(&ast.attrs);
            if let syn::Fields::Named(named) = &ast.fields {
                let mut slot = 0;
                for f in &named.named {
                    let Some(ident) = &f.ident else { continue };
                    let name = ident.to_string();
                    // A field's own rename wins over the container rule.
                    let rename = super::serde_attrs::serde_rename(f)
                        .or_else(|| rule.map(|r| r.apply(&name)));
                    fields.push(Arc::from(name.as_str()));
                    renames.push(rename.as_deref().map(Arc::from));
                    let ir = lower_type(&f.ty, self.resolver(), module, &[]);
                    coerce.push(ir.is_active().then(|| ir.clone()));
                    json.push(ir);
                    optional.push(matches!(
                        &f.ty,
                        syn::Type::Path(p)
                            if p.path.segments.last().is_some_and(|s| s.ident == "Option")
                    ));
                    key_map.insert(rename.unwrap_or(name), slot);
                    slot += 1;
                }
            }
            let shape = StructShape::typed(
                Arc::from(&**canon),
                self.resolver().type_id_of(canon),
                fields,
                renames,
            );
            out.insert(
                Arc::from(&**canon),
                Arc::new(StructInfo {
                    shape,
                    coerce,
                    json,
                    optional,
                    key_map,
                }),
            );
        }
        out
    }
}

// -- coercion ---------------------------------------------------------------

impl Vm {
    /// Turn a dynamic value into `ty` when it reaches a known struct, walking
    /// `Vec<T>` and `Option<T>`. The `Value` twin of `coerce_value` in
    /// eval.rs.
    pub(super) fn coerce_value(&self, value: Value, ty: &TypeIr) -> Value {
        match ty {
            TypeIr::Dynamic | TypeIr::Generic(_) | TypeIr::MapValue(_) => value,
            TypeIr::Vec(inner) => {
                let Value::Vec(items) = &value else {
                    return value;
                };
                match &**inner {
                    // A struct element type resolves once for the whole
                    // vector, and a primitive element type needs no work.
                    TypeIr::Struct(canon) => match self.structs.get(&**canon) {
                        Some(info) => Value::vec(
                            items
                                .lock()
                                .iter()
                                .map(|v| match v {
                                    Value::Map(m, _) => self.struct_from_map(info, &m.lock()),
                                    other => other.clone(),
                                })
                                .collect(),
                        ),
                        None => value,
                    },
                    TypeIr::Vec(_) | TypeIr::Option(_) | TypeIr::Set(_) => {
                        let out = items
                            .lock()
                            .iter()
                            .map(|v| self.coerce_value(v.clone(), inner))
                            .collect();
                        Value::vec(out)
                    }
                    TypeIr::Dynamic | TypeIr::Generic(_) | TypeIr::MapValue(_) => value,
                }
            }
            TypeIr::Set(inner) => {
                // A map-shaped value only needs the set tag. A `collect()`
                // lands here as a Vec and packs into the shared map storage.
                if let Value::Map(m, _) = &value {
                    return Value::Map(m.clone(), super::value::MapKind::Set);
                }
                let Value::Vec(items) = &value else {
                    return value;
                };
                let mut set = indexmap::IndexMap::default();
                for v in items.lock().iter() {
                    // An element that cannot be a key leaves the value alone,
                    // the give-up path every other coercion takes.
                    let Some(key) = self.coerce_value(v.clone(), inner).into_key() else {
                        return value.clone();
                    };
                    set.insert(key, Value::Unit);
                }
                Value::set_of(set)
            }
            TypeIr::Option(inner) => {
                if let Some(payload) = value.some_payload() {
                    return Value::some(self.coerce_value(payload, inner));
                }
                value
            }
            TypeIr::Struct(canon) => {
                if let Value::Map(map, _) = &value
                    && let Some(info) = self.structs.get(&**canon)
                {
                    return self.struct_from_map(info, &map.lock());
                }
                value
            }
        }
    }

    /// If `value` is `Ok(x)` coerce `x`, otherwise coerce `value` directly.
    pub(super) fn coerce_result(&self, value: Value, ty: &TypeIr) -> Value {
        if let Value::Enum { def, variant, data } = &value
            && def.kind == EnumKind::Result
            && *variant == OK
        {
            let inner = data.lock().first().cloned().unwrap_or(Value::Unit);
            return Value::ok(self.coerce_value(inner, ty));
        }
        self.coerce_value(value, ty)
    }

    fn struct_from_map(&self, info: &StructInfo, map: &indexmap::IndexMap<MapKey, Value>) -> Value {
        let mut values = Vec::with_capacity(info.coerce.len());
        for (fname, ty) in info.shape.fields.iter().zip(&info.coerce) {
            let raw = map
                .get(&MapKey::Str((&**fname).into()))
                .cloned()
                .unwrap_or_else(Value::none);
            let coerced = match ty {
                Some(t) => self.coerce_value(raw, t),
                None => raw,
            };
            values.push(coerced);
        }
        Value::structure(info.shape.clone(), values)
    }

    /// Lower a turbofish type into a parse plan, the `Value` twin of
    /// `json_plan` in `json_bridge.rs`. `building` guards recursive structs.
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
            // A set parses as a list first; the annotation coercion packs it
            // into the shared map storage afterwards. The elements still
            // parse with their own plan.
            TypeIr::Vec(inner) | TypeIr::Set(inner) => {
                JsonPlan::Vec(Box::new(self.json_plan(inner, building, tenv)))
            }
            TypeIr::Option(inner) => self.json_plan(inner, building, tenv),
            TypeIr::MapValue(inner) => {
                JsonPlan::Map(Box::new(self.json_plan(inner, building, tenv)))
            }
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

    /// `serde_json::from_str::<T>` with a known target type, the `Value`
    /// twin of `typed_from_str`.
    pub(super) fn typed_from_str(
        &self,
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
        Ok(match parse_json_planned(text, &plan) {
            Ok(v) => Value::ok(v),
            Err(e) => Value::err(Value::str(e.to_string())),
        })
    }
}

// -- parsing ----------------------------------------------------------------

pub(super) enum JsonPlan {
    Dynamic,
    Vec(Box<JsonPlan>),
    Map(Box<JsonPlan>),
    Struct(Arc<StructPlan>),
}

pub(super) struct StructPlan {
    info: Arc<StructInfo>,
    /// One plan per shape field, same order.
    fields: Vec<JsonPlan>,
}

/// Object keys repeat for every array element, so each parse interns them,
/// mirroring `JsonKeys` in `json_bridge.rs`. The parse runs on one thread, so
/// a `RefCell` is fine even though the values are `Send`.
type JsonKeys = RefCell<FxHashMap<String, RsStr>>;

pub(super) fn parse_json(text: &str) -> std::result::Result<Value, serde_json::Error> {
    use serde::de::DeserializeSeed;
    let mut de = serde_json::Deserializer::from_str(text);
    let keys = RefCell::new(FxHashMap::default());
    let v = PlanSeed {
        plan: &JsonPlan::Dynamic,
        keys: &keys,
    }
    .deserialize(&mut de)?;
    de.end()?;
    Ok(v)
}

fn parse_json_planned(
    text: &str,
    plan: &JsonPlan,
) -> std::result::Result<Value, serde_json::Error> {
    use serde::de::DeserializeSeed;
    let mut de = serde_json::Deserializer::from_str(text);
    let keys = RefCell::new(FxHashMap::default());
    let v = PlanSeed { plan, keys: &keys }.deserialize(&mut de)?;
    de.end()?;
    Ok(v)
}

struct PlanSeed<'a> {
    plan: &'a JsonPlan,
    keys: &'a JsonKeys,
}

impl<'de> serde::de::DeserializeSeed<'de> for PlanSeed<'_> {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        d: D,
    ) -> std::result::Result<Value, D::Error> {
        d.deserialize_any(PlanVisitor {
            plan: self.plan,
            keys: self.keys,
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

/// Key seed that resolves an object key to its slot in the target struct,
/// without allocating. Unknown keys come back as None and are skipped.
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

struct PlanVisitor<'a> {
    plan: &'a JsonPlan,
    keys: &'a JsonKeys,
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
        // A u64 past `i64::MAX` is an exact json integer, so it keeps its
        // width instead of turning into a float that cannot hold it.
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
        let elem = match self.plan {
            JsonPlan::Vec(p) => &**p,
            _ => &JsonPlan::Dynamic,
        };
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(v) = seq.next_element_seed(PlanSeed {
            plan: elem,
            keys: self.keys,
        })? {
            items.push(v);
        }
        Ok(Value::vec(items))
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
                                keys: self.keys,
                            })?;
                            // An Option field wraps a present, non-null value
                            // in Some so a `match Some(x)` matches.
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
                // A required field with no key in the json fails the parse,
                // like real serde, instead of binding a hole that only
                // explodes later. Option fields stay None.
                missing_field(&filled, &sp.info.optional, &sp.info.key_map)?;
                Ok(Value::structure(sp.info.shape.clone(), values))
            }
            plan => {
                let elem = match plan {
                    JsonPlan::Map(p) => &**p,
                    _ => &JsonPlan::Dynamic,
                };
                let mut map = indexmap::IndexMap::default();
                while let Some(k) = access.next_key_seed(KeySeed { keys: self.keys })? {
                    map.insert(
                        MapKey::Str(k),
                        access.next_value_seed(PlanSeed {
                            plan: elem,
                            keys: self.keys,
                        })?,
                    );
                }
                Ok(Value::map_of(map))
            }
        }
    }
}

// -- serialization ----------------------------------------------------------

/// A `serde_json::Value` as an interpreter value, for the toml and yaml
/// bridges that parse through `serde_json`'s model. Null maps to None, the same
/// mapping the json parser uses.
pub(super) fn json_to_pvalue(v: serde_json::Value) -> Value {
    use serde_json::Value as J;
    match v {
        J::Null => Value::none(),
        J::Bool(b) => Value::Bool(b),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(u) = n.as_u64() {
                Value::int_of_width(i128::from(u), super::numeric::IntWidth::U64)
            } else {
                Value::Float(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        J::String(s) => Value::str(s),
        J::Array(items) => Value::vec(items.into_iter().map(json_to_pvalue).collect()),
        J::Object(map) => {
            let mut out = indexmap::IndexMap::default();
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
    use serde_json::Value as J;
    Ok(match v {
        Value::Unit => J::Null,
        Value::Bool(b) => J::Bool(*b),
        Value::Int(i) => J::Number(serde_json::Number::from(*i)),
        Value::IntW(..) => {
            let (value, _) = v.int_parts().unwrap();
            match i64::try_from(value) {
                Ok(small) => J::Number(serde_json::Number::from(small)),
                Err(_) => J::Number(serde_json::Number::from(
                    u64::try_from(value).expect("width-tagged value fits u64"),
                )),
            }
        }
        // Serde represents a 128-bit integer as a number only while it fits
        // the u64/i64 json range, the same bound `serde_json` enforces.
        Value::Big(raw, w) => {
            let as_i64 = if *w == super::numeric::IntWidth::U128 {
                i64::try_from(raw.cast_unsigned()).map_err(|_| ())
            } else {
                i64::try_from(*raw).map_err(|_| ())
            };
            match as_i64 {
                Ok(small) => J::Number(serde_json::Number::from(small)),
                Err(()) => bail!("128-bit integer does not fit a json number"),
            }
        }
        Value::Float(f) => serde_json::Number::from_f64(*f).map_or(J::Null, J::Number),
        Value::F32(f) => serde_json::Number::from_f64(f64::from(*f)).map_or(J::Null, J::Number),
        Value::Char(c) => J::String(c.to_string()),
        Value::Str(s) => J::String(s.to_string()),
        Value::Vec(items) | Value::Tuple(items) => J::Array(
            items
                .lock()
                .iter()
                .map(pvalue_to_json)
                .collect::<Result<_>>()?,
        ),
        Value::Map(map, _) => {
            let mut obj = serde_json::Map::default();
            for (k, val) in map.lock().iter() {
                obj.insert(k.to_value().display(), pvalue_to_json(val)?);
            }
            J::Object(obj)
        }
        Value::Struct(s) => {
            let mut obj = serde_json::Map::default();
            let values = s.values.lock();
            for (slot, (field, val)) in s.shape.fields.iter().zip(values.iter()).enumerate() {
                let key = s
                    .shape
                    .renames
                    .get(slot)
                    .and_then(Option::as_ref)
                    .unwrap_or(field);
                obj.insert(key.to_string(), pvalue_to_json(val)?);
            }
            J::Object(obj)
        }
        Value::Enum { def, variant, data } => {
            let payload = data.lock().clone();
            if def.kind == EnumKind::Option {
                if *variant == SOME {
                    pvalue_to_json(&payload[0])?
                } else {
                    J::Null
                }
            } else if payload.is_empty() {
                J::String(def.variant_name(*variant).to_string())
            } else {
                let mut obj = serde_json::Map::default();
                obj.insert(
                    def.variant_name(*variant).to_string(),
                    J::Array(payload.iter().map(pvalue_to_json).collect::<Result<_>>()?),
                );
                J::Object(obj)
            }
        }
        Value::Range { .. } => bail!("cannot serialize a range to json"),
        Value::Closure(_) => bail!("cannot serialize a closure to json"),
        // Serde serializes Rc, Arc, RefCell, and Mutex by content.
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

/// The `serde_json` free functions on the dynamic path, `from_str` with no
/// type information plus `to_string` and `to_string_pretty`.
pub(super) fn bridge_serde_json(id: P, args: &[Value]) -> Result<Value> {
    match id {
        P::SerdeJsonFromStr => {
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
        P::SerdeJsonToString | P::SerdeJsonToStringPretty => {
            let v = args.first().cloned().unwrap_or(Value::Unit);
            let j = pvalue_to_json(&v)?;
            let s = if id == P::SerdeJsonToStringPretty {
                serde_json::to_string_pretty(&j)?
            } else {
                serde_json::to_string(&j)?
            };
            Ok(Value::ok(Value::str(s)))
        }
        P::SerdeJsonToValue => {
            let v = args.first().cloned().unwrap_or(Value::Unit);
            Ok(Value::ok(json_to_pvalue(pvalue_to_json(&v)?)))
        }
        _ => bail!("unsupported serde_json function `{id}`"),
    }
}

/// Error out on a required struct field the json object never supplied.
fn missing_field<E: serde::de::Error>(
    filled: &[bool],
    optional: &[bool],
    key_map: &FxHashMap<String, usize>,
) -> std::result::Result<(), E> {
    for (i, done) in filled.iter().enumerate() {
        if *done || optional.get(i).copied().unwrap_or(false) {
            continue;
        }
        let key = key_map
            .iter()
            .find(|(_, slot)| **slot == i)
            .map_or("?", |(k, _)| k.as_str());
        return Err(E::custom(format!("missing field `{key}`")));
    }
    Ok(())
}
