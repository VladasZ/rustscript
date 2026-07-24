//! Direct serde based json parsing plus conversions between script
//! values and `serde_json::Value`. Split from `builtins.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Result, bail};

use rustc_hash::FxHashMap;

use super::Interp;
use super::typeir::{TypeIr, lower_type};
use super::value::{MapKey, RStr, StructShape, Value, map_with_capacity};

// -- serde_json bridge -----------------------------------------------------

/// Parse json text straight into a script value, skipping the intermediate
/// `serde_json::Value` tree that would otherwise be built and dropped.
pub(super) fn parse_json(text: &str) -> std::result::Result<Value, serde_json::Error> {
    use serde::de::DeserializeSeed;
    let mut de = serde_json::Deserializer::from_str(text);
    let keys = RefCell::new(FxHashMap::default());
    let v = JsonSeed { keys: &keys }.deserialize(&mut de)?;
    de.end()?;
    Ok(v)
}

/// Object keys repeat for every element of an array, so each parse keeps one
/// intern table and every repeat of a key shares the first `Rc`. That skips
/// the allocation and gives later map lookups pointer equality plus a warm
/// hash cache.
pub(super) type JsonKeys = RefCell<FxHashMap<String, Rc<RStr>>>;

pub(super) struct JsonSeed<'a> {
    keys: &'a JsonKeys,
}

impl<'de> serde::de::DeserializeSeed<'de> for JsonSeed<'_> {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        d: D,
    ) -> std::result::Result<Value, D::Error> {
        d.deserialize_any(JsonVisitor { keys: self.keys })
    }
}

pub(super) struct KeySeed<'a> {
    keys: &'a JsonKeys,
}

impl KeySeed<'_> {
    fn intern(&self, s: &str) -> Rc<RStr> {
        if let Some(rc) = self.keys.borrow().get(s) {
            return rc.clone();
        }
        let rc = RStr::new(s);
        self.keys.borrow_mut().insert(s.to_string(), rc.clone());
        rc
    }
}

impl<'de> serde::de::DeserializeSeed<'de> for KeySeed<'_> {
    type Value = Rc<RStr>;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        d: D,
    ) -> std::result::Result<Rc<RStr>, D::Error> {
        d.deserialize_str(self)
    }
}

impl<'de> serde::de::Visitor<'de> for KeySeed<'_> {
    type Value = Rc<RStr>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an object key")
    }

    fn visit_str<E: serde::de::Error>(self, s: &str) -> std::result::Result<Rc<RStr>, E> {
        Ok(self.intern(s))
    }

    fn visit_string<E: serde::de::Error>(self, s: String) -> std::result::Result<Rc<RStr>, E> {
        Ok(self.intern(&s))
    }
}

pub(super) struct JsonVisitor<'a> {
    keys: &'a JsonKeys,
}

impl<'de> serde::de::Visitor<'de> for JsonVisitor<'_> {
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
        Ok(match i64::try_from(u) {
            Ok(i) => Value::Int(i),
            Err(_) => Value::Float(u as f64),
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
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(v) = seq.next_element_seed(JsonSeed { keys: self.keys })? {
            items.push(v);
        }
        Ok(Value::vec(items))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(
        self,
        mut access: A,
    ) -> std::result::Result<Value, A::Error> {
        let mut map = map_with_capacity(access.size_hint().unwrap_or(0));
        while let Some(k) = access.next_key_seed(KeySeed { keys: self.keys })? {
            map.insert(
                MapKey::Str(k),
                access.next_value_seed(JsonSeed { keys: self.keys })?,
            );
        }
        Ok(Value::Map(Rc::new(RefCell::new(map))))
    }
}

// -- typed deserialization ---------------------------------------------------

/// What `from_str::<T>` should build while parsing, so a known struct target
/// goes straight into `Value::Struct` with no intermediate map and no
/// coercion pass afterwards.
pub(super) enum JsonPlan {
    /// No type information, parse like the untyped path.
    Dynamic,
    Vec(Box<JsonPlan>),
    Map(Box<JsonPlan>),
    Struct(Rc<StructPlan>),
}

pub(super) struct StructPlan {
    pub shape: Rc<StructShape>,
    /// One plan per shape field, same order.
    pub fields: Vec<JsonPlan>,
    /// Whether field i was declared `Option<T>`, so a present value is wrapped
    /// in `Some` and a missing key stays `None`.
    pub optional: Vec<bool>,
    /// Json object key to field slot. Holds the `#[serde(rename = "..")]` name
    /// when set, otherwise the field name, so camelCase keys map correctly.
    pub key_map: FxHashMap<String, usize>,
}

/// Read a field's `#[serde(rename = "..")]` value, if present.
pub(super) fn serde_rename(field: &syn::Field) -> Option<String> {
    let mut renamed = None;
    for attr in &field.attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        // parse_nested_meta walks the `serde(...)` list, e.g. `rename = "x"`.
        if attr
            .parse_nested_meta(|meta| {
                if meta.path.is_ident("rename")
                    && let Ok(value) = meta.value()
                    && let Ok(lit) = value.parse::<syn::LitStr>()
                {
                    renamed = Some(lit.value());
                }
                Ok(())
            })
            .is_err()
        {
            return None;
        }
    }
    renamed
}

/// Whether a type is spelled `Option<..>` at the top level.
fn is_option(ty: &syn::Type) -> bool {
    if let syn::Type::Path(p) = ty
        && let Some(seg) = p.path.segments.last()
    {
        return seg.ident == "Option";
    }
    false
}

impl Interp {
    /// Lower a turbofish type into a parse plan. `building` guards against
    /// recursive struct definitions, which fall back to dynamic parsing.
    pub(super) fn json_plan(
        &self,
        ty: &TypeIr,
        building: &mut Vec<String>,
        tenv: &[(Rc<str>, TypeIr)],
    ) -> JsonPlan {
        match ty {
            TypeIr::Dynamic => JsonPlan::Dynamic,
            // A generic parameter the caller bound by turbofish resolves to
            // its concrete type, already lowered in the caller's module.
            TypeIr::Generic(name) => match tenv.iter().find(|(n, _)| **n == **name) {
                Some((_, bound)) => self.json_plan(bound, building, tenv),
                None => JsonPlan::Dynamic,
            },
            TypeIr::Vec(inner) => JsonPlan::Vec(Box::new(self.json_plan(inner, building, tenv))),
            TypeIr::Option(inner) => self.json_plan(inner, building, tenv),
            TypeIr::MapValue(inner) => {
                JsonPlan::Map(Box::new(self.json_plan(inner, building, tenv)))
            }
            TypeIr::Struct(canon) => {
                if building.iter().any(|b| b.as_str() == &**canon) {
                    return JsonPlan::Dynamic;
                }
                let Some(shape) = self.struct_shape(canon) else {
                    return JsonPlan::Dynamic;
                };
                let Some(def) = self.structs().get(&**canon) else {
                    return JsonPlan::Dynamic;
                };
                let def_module = def.module;
                let def = def.ast.clone();
                building.push(canon.to_string());
                let mut fields = Vec::with_capacity(shape.runtime.fields.len());
                let mut optional = Vec::with_capacity(shape.runtime.fields.len());
                let mut key_map = FxHashMap::default();
                if let syn::Fields::Named(named) = &def.fields {
                    let mut slot = 0;
                    for f in &named.named {
                        let Some(ident) = &f.ident else {
                            continue;
                        };
                        // Field types resolve where the struct is declared and
                        // are concrete, so no caller type env applies here.
                        let fir = lower_type(&f.ty, self.resolver(), def_module, &[]);
                        fields.push(self.json_plan(&fir, building, &[]));
                        optional.push(is_option(&f.ty));
                        let key = serde_rename(f).unwrap_or_else(|| ident.to_string());
                        key_map.insert(key, slot);
                        slot += 1;
                    }
                }
                building.pop();
                JsonPlan::Struct(Rc::new(StructPlan {
                    shape: shape.runtime.clone(),
                    fields,
                    optional,
                    key_map,
                }))
            }
        }
    }

    /// `serde_json::from_str::<T>` with a known target type. Parses straight
    /// into typed values, so no coercion pass runs afterwards.
    pub(super) fn typed_from_str(
        &self,
        args: &[Value],
        ty: &TypeIr,
        tenv: &[(Rc<str>, TypeIr)],
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

pub(super) fn parse_json_planned(
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
        Ok(match i64::try_from(u) {
            Ok(i) => Value::Int(i),
            Err(_) => Value::Float(u as f64),
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
                // Missing fields become None, like the coercion pass did.
                let mut values: Vec<Value> =
                    (0..sp.shape.fields.len()).map(|_| Value::none()).collect();
                while let Some(slot) = access.next_key_seed(FieldSeed {
                    key_map: &sp.key_map,
                })? {
                    match slot {
                        Some(i) => {
                            let v = access.next_value_seed(PlanSeed {
                                plan: &sp.fields[i],
                                keys: self.keys,
                            })?;
                            // An Option field wraps a present, non-null value in
                            // Some so a `match Some(x)` in the script matches.
                            values[i] = if sp.optional[i] && !v.is_none_value() {
                                Value::some(v)
                            } else {
                                v
                            };
                        }
                        None => {
                            access.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(Value::structure(sp.shape.clone(), values))
            }
            plan => {
                let elem = match plan {
                    JsonPlan::Map(p) => &**p,
                    _ => &JsonPlan::Dynamic,
                };
                let mut map = map_with_capacity(access.size_hint().unwrap_or(0));
                while let Some(k) = access.next_key_seed(KeySeed { keys: self.keys })? {
                    map.insert(
                        MapKey::Str(k),
                        access.next_value_seed(PlanSeed {
                            plan: elem,
                            keys: self.keys,
                        })?,
                    );
                }
                Ok(Value::Map(Rc::new(RefCell::new(map))))
            }
        }
    }
}

pub(super) fn bridge_serde_json(func: &str, args: &[Value]) -> Result<Value> {
    match func {
        "from_str" => {
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
        "to_string" | "to_string_pretty" => {
            let v = args.first().cloned().unwrap_or(Value::Unit);
            let j = value_to_json(&v)?;
            let s = if func == "to_string_pretty" {
                serde_json::to_string_pretty(&j)?
            } else {
                serde_json::to_string(&j)?
            };
            Ok(Value::ok(Value::str(s)))
        }
        "to_value" => {
            let v = args.first().cloned().unwrap_or(Value::Unit);
            Ok(Value::ok(json_to_value(value_to_json(&v)?)))
        }
        other => bail!("unsupported serde_json function `{other}`"),
    }
}

/// Consumes the parsed tree so strings move into values instead of cloning.
pub(super) fn json_to_value(j: serde_json::Value) -> Value {
    match j {
        serde_json::Value::Null => Value::none(),
        serde_json::Value::Bool(b) => Value::Bool(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else {
                Value::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Value::str(s),
        serde_json::Value::Array(a) => Value::vec(a.into_iter().map(json_to_value).collect()),
        serde_json::Value::Object(o) => {
            let mut map = map_with_capacity(o.len());
            for (k, v) in o {
                map.insert(MapKey::Str(RStr::new(k)), json_to_value(v));
            }
            Value::Map(Rc::new(RefCell::new(map)))
        }
    }
}

pub(super) fn value_to_json(v: &Value) -> Result<serde_json::Value> {
    use serde_json::Value as J;
    Ok(match v {
        Value::Unit => J::Null,
        Value::Bool(b) => J::Bool(*b),
        Value::Int(i) => J::Number(serde_json::Number::from(*i)),
        Value::IntW(..) => {
            let (value, _) = v.int_parts().unwrap();
            match i64::try_from(value) {
                Ok(small) => J::Number(serde_json::Number::from(small)),
                Err(_) => J::Number(serde_json::Number::from(value as u64)),
            }
        }
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(J::Number)
            .unwrap_or(J::Null),
        Value::F32(f) => serde_json::Number::from_f64(f64::from(*f))
            .map(J::Number)
            .unwrap_or(J::Null),
        Value::Char(c) => J::String(c.to_string()),
        Value::Str(s) => J::String(s.to_string()),
        Value::Vec(items) | Value::Tuple(items) => J::Array(
            items
                .borrow()
                .iter()
                .map(value_to_json)
                .collect::<Result<_>>()?,
        ),
        Value::Map(map) => {
            let mut obj = serde_json::Map::default();
            for (k, val) in map.borrow().iter() {
                obj.insert(k.to_value().display(), value_to_json(val)?);
            }
            J::Object(obj)
        }
        Value::Struct(s) => {
            let mut obj = serde_json::Map::default();
            let values = s.values.borrow();
            for (slot, (field, val)) in s.shape.fields.iter().zip(values.iter()).enumerate() {
                let key = s
                    .shape
                    .renames
                    .get(slot)
                    .and_then(Option::as_ref)
                    .unwrap_or(field);
                obj.insert(key.to_string(), value_to_json(val)?);
            }
            J::Object(obj)
        }
        Value::Enum {
            enum_name,
            variant,
            data,
        } => {
            if &**enum_name == "Option" {
                match &**variant {
                    "Some" => value_to_json(&data[0])?,
                    _ => J::Null,
                }
            } else {
                if data.is_empty() {
                    J::String(variant.to_string())
                } else {
                    let mut obj = serde_json::Map::default();
                    obj.insert(
                        variant.to_string(),
                        J::Array(data.iter().map(value_to_json).collect::<Result<_>>()?),
                    );
                    J::Object(obj)
                }
            }
        }
        Value::Range { .. } => bail!("cannot serialize a range to json"),
        Value::Closure(_) => bail!("cannot serialize a closure to json"),
        Value::Ref(reference) => {
            let Some(value) = reference.get() else {
                bail!("cannot serialize a dangling reference to json");
            };
            value_to_json(&value)?
        }
        Value::Native(n) => bail!("cannot serialize a {} to json", n.borrow().type_name()),
    })
}
