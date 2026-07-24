//! serde_json for the parallel engine: dynamic and typed parsing straight
//! into `PValue`, serialization back to json text, and the coercion pass for
//! annotated lets. The `PValue` twin of `json_bridge.rs` and the coercion
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
use super::pvalue::{PKey, PStructShape, PValue};
use super::pvm::PInterp;
use super::typeir::{TypeIr, lower_type};

/// Everything the parallel engine needs to know about one user struct,
/// precomputed at load: the runtime layout, the lowered field types for
/// coercion and typed json, and the json key mapping with serde renames.
pub struct PStructInfo {
    pub shape: Arc<PStructShape>,
    /// Per field, its lowered type when coercion can change the value.
    pub coerce: Vec<Option<TypeIr>>,
    /// Per field, its lowered type for json planning.
    pub json: Vec<TypeIr>,
    /// Whether field i was declared `Option<T>` in the source.
    pub optional: Vec<bool>,
    /// Json object key to field slot, `#[serde(rename)]` applied.
    pub key_map: FxHashMap<String, usize>,
}

pub type PStructs = HashMap<Arc<str>, Arc<PStructInfo>>;

impl Interp {
    /// Build the parallel struct table from the AST, once at load on the main
    /// thread. Mirrors what `struct_shape` and `json_plan` read lazily on the
    /// fast engine.
    pub(super) fn build_pstructs(&self) -> PStructs {
        let mut out = PStructs::default();
        for (canon, def) in self.structs() {
            let module = def.module;
            let ast = def.ast.clone();
            let mut fields: Vec<Arc<str>> = Vec::new();
            let mut renames: Vec<Option<Arc<str>>> = Vec::new();
            let mut coerce = Vec::new();
            let mut json = Vec::new();
            let mut optional = Vec::new();
            let mut key_map = FxHashMap::default();
            if let syn::Fields::Named(named) = &ast.fields {
                let mut slot = 0;
                for f in &named.named {
                    let Some(ident) = &f.ident else { continue };
                    let name = ident.to_string();
                    let rename = super::json_bridge::serde_rename(f);
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
            let shape = Arc::new(PStructShape {
                name: Arc::from(&**canon),
                fields,
                renames,
            });
            out.insert(
                Arc::from(&**canon),
                Arc::new(PStructInfo {
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

impl PInterp {
    /// Turn a dynamic value into `ty` when it reaches a known struct, walking
    /// `Vec<T>` and `Option<T>`. The `PValue` twin of `coerce_value` in
    /// eval.rs.
    pub(super) fn coerce_value(&self, value: PValue, ty: &TypeIr) -> PValue {
        match ty {
            TypeIr::Dynamic | TypeIr::Generic(_) | TypeIr::MapValue(_) => value,
            TypeIr::Vec(inner) => {
                let PValue::Vec(items) = &value else {
                    return value;
                };
                match &**inner {
                    // A struct element type resolves once for the whole
                    // vector, and a primitive element type needs no work.
                    TypeIr::Struct(canon) => match self.structs.get(&**canon) {
                        Some(info) => PValue::vec(
                            items
                                .lock()
                                .iter()
                                .map(|v| match v {
                                    PValue::Map(m) => self.struct_from_map(info, &m.lock()),
                                    other => other.clone(),
                                })
                                .collect(),
                        ),
                        None => value,
                    },
                    TypeIr::Vec(_) | TypeIr::Option(_) => {
                        let out = items
                            .lock()
                            .iter()
                            .map(|v| self.coerce_value(v.clone(), inner))
                            .collect();
                        PValue::vec(out)
                    }
                    TypeIr::Dynamic | TypeIr::Generic(_) | TypeIr::MapValue(_) => value,
                }
            }
            TypeIr::Option(inner) => {
                if let PValue::Enum {
                    enum_name,
                    variant,
                    data,
                } = &value
                    && &**enum_name == "Option"
                    && &**variant == "Some"
                {
                    let coerced =
                        self.coerce_value(data.first().cloned().unwrap_or(PValue::Unit), inner);
                    return PValue::some(coerced);
                }
                value
            }
            TypeIr::Struct(canon) => {
                if let PValue::Map(map) = &value
                    && let Some(info) = self.structs.get(&**canon)
                {
                    return self.struct_from_map(info, &map.lock());
                }
                value
            }
        }
    }

    /// If `value` is `Ok(x)` coerce `x`, otherwise coerce `value` directly.
    pub(super) fn coerce_result(&self, value: PValue, ty: &TypeIr) -> PValue {
        if let PValue::Enum {
            enum_name,
            variant,
            data,
        } = &value
            && &**enum_name == "Result"
            && &**variant == "Ok"
        {
            let inner = data.first().cloned().unwrap_or(PValue::Unit);
            return PValue::ok(self.coerce_value(inner, ty));
        }
        self.coerce_value(value, ty)
    }

    fn struct_from_map(
        &self,
        info: &PStructInfo,
        map: &indexmap::IndexMap<PKey, PValue>,
    ) -> PValue {
        let mut values = Vec::with_capacity(info.coerce.len());
        for (fname, ty) in info.shape.fields.iter().zip(&info.coerce) {
            let raw = map
                .get(&PKey::Str(fname.clone()))
                .cloned()
                .unwrap_or_else(PValue::none);
            let coerced = match ty {
                Some(t) => self.coerce_value(raw, t),
                None => raw,
            };
            values.push(coerced);
        }
        PValue::structure(info.shape.clone(), values)
    }

    /// Lower a turbofish type into a parse plan, the `PValue` twin of
    /// `json_plan` in json_bridge.rs. `building` guards recursive structs.
    pub(super) fn json_plan(
        &self,
        ty: &TypeIr,
        building: &mut Vec<String>,
        tenv: &[(Arc<str>, TypeIr)],
    ) -> PJsonPlan {
        match ty {
            TypeIr::Dynamic => PJsonPlan::Dynamic,
            TypeIr::Generic(name) => match tenv.iter().find(|(n, _)| **n == **name) {
                Some((_, bound)) => self.json_plan(bound, building, tenv),
                None => PJsonPlan::Dynamic,
            },
            TypeIr::Vec(inner) => PJsonPlan::Vec(Box::new(self.json_plan(inner, building, tenv))),
            TypeIr::Option(inner) => self.json_plan(inner, building, tenv),
            TypeIr::MapValue(inner) => {
                PJsonPlan::Map(Box::new(self.json_plan(inner, building, tenv)))
            }
            TypeIr::Struct(canon) => {
                if building.iter().any(|b| b.as_str() == &**canon) {
                    return PJsonPlan::Dynamic;
                }
                let Some(info) = self.structs.get(&**canon) else {
                    return PJsonPlan::Dynamic;
                };
                building.push(canon.to_string());
                let fields = info
                    .json
                    .iter()
                    .map(|fir| self.json_plan(fir, building, &[]))
                    .collect();
                building.pop();
                PJsonPlan::Struct(Arc::new(PStructPlan {
                    info: info.clone(),
                    fields,
                }))
            }
        }
    }

    /// `serde_json::from_str::<T>` with a known target type, the `PValue`
    /// twin of `typed_from_str`.
    pub(super) fn typed_from_str(
        &self,
        args: &[PValue],
        ty: &TypeIr,
        tenv: &[(Arc<str>, TypeIr)],
    ) -> Result<PValue> {
        let owned;
        let text: &str = match args.first() {
            Some(PValue::Str(s)) => s,
            Some(other) => {
                owned = other.display();
                &owned
            }
            None => bail!("from_str needs a string"),
        };
        let plan = self.json_plan(ty, &mut Vec::new(), tenv);
        Ok(match parse_json_planned(text, &plan) {
            Ok(v) => PValue::ok(v),
            Err(e) => PValue::err(PValue::str(e.to_string())),
        })
    }
}

// -- parsing ----------------------------------------------------------------

pub(super) enum PJsonPlan {
    Dynamic,
    Vec(Box<PJsonPlan>),
    Map(Box<PJsonPlan>),
    Struct(Arc<PStructPlan>),
}

pub(super) struct PStructPlan {
    info: Arc<PStructInfo>,
    /// One plan per shape field, same order.
    fields: Vec<PJsonPlan>,
}

/// Object keys repeat for every array element, so each parse interns them,
/// mirroring `JsonKeys` in json_bridge.rs. The parse runs on one thread, so
/// a `RefCell` is fine even though the values are `Send`.
type PJsonKeys = RefCell<FxHashMap<String, Arc<str>>>;

pub(super) fn parse_json(text: &str) -> std::result::Result<PValue, serde_json::Error> {
    use serde::de::DeserializeSeed;
    let mut de = serde_json::Deserializer::from_str(text);
    let keys = RefCell::new(FxHashMap::default());
    let v = PlanSeed {
        plan: &PJsonPlan::Dynamic,
        keys: &keys,
    }
    .deserialize(&mut de)?;
    de.end()?;
    Ok(v)
}

fn parse_json_planned(
    text: &str,
    plan: &PJsonPlan,
) -> std::result::Result<PValue, serde_json::Error> {
    use serde::de::DeserializeSeed;
    let mut de = serde_json::Deserializer::from_str(text);
    let keys = RefCell::new(FxHashMap::default());
    let v = PlanSeed { plan, keys: &keys }.deserialize(&mut de)?;
    de.end()?;
    Ok(v)
}

struct PlanSeed<'a> {
    plan: &'a PJsonPlan,
    keys: &'a PJsonKeys,
}

impl<'de> serde::de::DeserializeSeed<'de> for PlanSeed<'_> {
    type Value = PValue;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        d: D,
    ) -> std::result::Result<PValue, D::Error> {
        d.deserialize_any(PlanVisitor {
            plan: self.plan,
            keys: self.keys,
        })
    }
}

struct KeySeed<'a> {
    keys: &'a PJsonKeys,
}

impl KeySeed<'_> {
    fn intern(&self, s: &str) -> Arc<str> {
        if let Some(k) = self.keys.borrow().get(s) {
            return k.clone();
        }
        let k: Arc<str> = Arc::from(s);
        self.keys.borrow_mut().insert(s.to_string(), k.clone());
        k
    }
}

impl<'de> serde::de::DeserializeSeed<'de> for KeySeed<'_> {
    type Value = Arc<str>;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        d: D,
    ) -> std::result::Result<Arc<str>, D::Error> {
        d.deserialize_str(self)
    }
}

impl<'de> serde::de::Visitor<'de> for KeySeed<'_> {
    type Value = Arc<str>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an object key")
    }

    fn visit_str<E: serde::de::Error>(self, s: &str) -> std::result::Result<Arc<str>, E> {
        Ok(self.intern(s))
    }

    fn visit_string<E: serde::de::Error>(self, s: String) -> std::result::Result<Arc<str>, E> {
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

impl<'de> serde::de::Visitor<'de> for FieldSeed<'_> {
    type Value = Option<usize>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an object key")
    }

    fn visit_str<E: serde::de::Error>(self, s: &str) -> std::result::Result<Option<usize>, E> {
        Ok(self.key_map.get(s).copied())
    }
}

struct PlanVisitor<'a> {
    plan: &'a PJsonPlan,
    keys: &'a PJsonKeys,
}

impl<'de> serde::de::Visitor<'de> for PlanVisitor<'_> {
    type Value = PValue;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a json value")
    }

    fn visit_bool<E>(self, b: bool) -> std::result::Result<PValue, E> {
        Ok(PValue::Bool(b))
    }

    fn visit_i64<E>(self, i: i64) -> std::result::Result<PValue, E> {
        Ok(PValue::Int(i))
    }

    fn visit_u64<E>(self, u: u64) -> std::result::Result<PValue, E> {
        Ok(match i64::try_from(u) {
            Ok(i) => PValue::Int(i),
            Err(_) => PValue::Float(u as f64),
        })
    }

    fn visit_f64<E>(self, f: f64) -> std::result::Result<PValue, E> {
        Ok(PValue::Float(f))
    }

    fn visit_str<E>(self, s: &str) -> std::result::Result<PValue, E> {
        Ok(PValue::str(s))
    }

    fn visit_string<E>(self, s: String) -> std::result::Result<PValue, E> {
        Ok(PValue::str(s))
    }

    fn visit_unit<E>(self) -> std::result::Result<PValue, E> {
        Ok(PValue::none())
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(
        self,
        mut seq: A,
    ) -> std::result::Result<PValue, A::Error> {
        let elem = match self.plan {
            PJsonPlan::Vec(p) => &**p,
            _ => &PJsonPlan::Dynamic,
        };
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(v) = seq.next_element_seed(PlanSeed {
            plan: elem,
            keys: self.keys,
        })? {
            items.push(v);
        }
        Ok(PValue::vec(items))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(
        self,
        mut access: A,
    ) -> std::result::Result<PValue, A::Error> {
        match self.plan {
            PJsonPlan::Struct(sp) => {
                // Missing fields become None, like the coercion pass did.
                let mut values: Vec<PValue> = (0..sp.info.shape.fields.len())
                    .map(|_| PValue::none())
                    .collect();
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
                                PValue::some(v)
                            } else {
                                v
                            };
                        }
                        None => {
                            access.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(PValue::structure(sp.info.shape.clone(), values))
            }
            plan => {
                let elem = match plan {
                    PJsonPlan::Map(p) => &**p,
                    _ => &PJsonPlan::Dynamic,
                };
                let mut map = indexmap::IndexMap::default();
                while let Some(k) = access.next_key_seed(KeySeed { keys: self.keys })? {
                    map.insert(
                        PKey::Str(k),
                        access.next_value_seed(PlanSeed {
                            plan: elem,
                            keys: self.keys,
                        })?,
                    );
                }
                Ok(PValue::Map(Arc::new(parking_lot::Mutex::new(map))))
            }
        }
    }
}

// -- serialization ----------------------------------------------------------

pub(super) fn pvalue_to_json(v: &PValue) -> Result<serde_json::Value> {
    use serde_json::Value as J;
    Ok(match v {
        PValue::Unit => J::Null,
        PValue::Bool(b) => J::Bool(*b),
        PValue::Int(i) => J::Number(serde_json::Number::from(*i)),
        PValue::IntW(..) => {
            let (value, _) = v.int_parts().unwrap();
            match i64::try_from(value) {
                Ok(small) => J::Number(serde_json::Number::from(small)),
                Err(_) => J::Number(serde_json::Number::from(value as u64)),
            }
        }
        PValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(J::Number)
            .unwrap_or(J::Null),
        PValue::F32(f) => serde_json::Number::from_f64(f64::from(*f))
            .map(J::Number)
            .unwrap_or(J::Null),
        PValue::Char(c) => J::String(c.to_string()),
        PValue::Str(s) => J::String(s.to_string()),
        PValue::Vec(items) | PValue::Tuple(items) => J::Array(
            items
                .lock()
                .iter()
                .map(pvalue_to_json)
                .collect::<Result<_>>()?,
        ),
        PValue::Map(map) => {
            let mut obj = serde_json::Map::default();
            for (k, val) in map.lock().iter() {
                obj.insert(k.to_value().display(), pvalue_to_json(val)?);
            }
            J::Object(obj)
        }
        PValue::Struct(s) => {
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
        PValue::Enum {
            enum_name,
            variant,
            data,
        } => {
            if &**enum_name == "Option" {
                match &**variant {
                    "Some" => pvalue_to_json(&data[0])?,
                    _ => J::Null,
                }
            } else if data.is_empty() {
                J::String(variant.to_string())
            } else {
                let mut obj = serde_json::Map::default();
                obj.insert(
                    variant.to_string(),
                    J::Array(data.iter().map(pvalue_to_json).collect::<Result<_>>()?),
                );
                J::Object(obj)
            }
        }
        PValue::Range { .. } => bail!("cannot serialize a range to json"),
        PValue::Closure(_) => bail!("cannot serialize a closure to json"),
        PValue::Ref(reference) => {
            let Some(value) = reference.get() else {
                bail!("cannot serialize a dangling reference to json");
            };
            pvalue_to_json(&value)?
        }
        PValue::Native(n) => bail!("cannot serialize a {} to json", n.lock().type_name()),
    })
}

/// The `serde_json` free functions on the dynamic path, `from_str` with no
/// type information plus `to_string` and `to_string_pretty`.
pub(super) fn bridge_serde_json(func: &str, args: &[PValue]) -> Result<PValue> {
    match func {
        "from_str" => {
            let owned;
            let s: &str = match args.first() {
                Some(PValue::Str(s)) => s,
                Some(other) => {
                    owned = other.display();
                    &owned
                }
                None => bail!("from_str needs a string"),
            };
            match parse_json(s) {
                Ok(v) => Ok(PValue::ok(v)),
                Err(e) => Ok(PValue::err(PValue::str(e.to_string()))),
            }
        }
        "to_string" | "to_string_pretty" => {
            let v = args.first().cloned().unwrap_or(PValue::Unit);
            let j = pvalue_to_json(&v)?;
            let s = if func == "to_string_pretty" {
                serde_json::to_string_pretty(&j)?
            } else {
                serde_json::to_string(&j)?
            };
            Ok(PValue::ok(PValue::str(s)))
        }
        other => bail!("unsupported serde_json function `{other}` in tokio mode"),
    }
}
