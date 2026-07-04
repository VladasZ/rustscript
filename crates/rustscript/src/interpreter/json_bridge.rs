//! Direct serde based json parsing plus conversions between script
//! values and `serde_json::Value`. Split from `builtins.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Result, bail};

use rustc_hash::FxHashMap;

use super::value::{MapKey, RStr, StructShape, Value, map_with_capacity};
use super::Interp;



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

    fn visit_string<E: serde::de::Error>(
        self,
        s: String,
    ) -> std::result::Result<Rc<RStr>, E> {
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
            map.insert(MapKey::Str(k), access.next_value_seed(JsonSeed { keys: self.keys })?);
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
}

impl Interp {
    /// Lower a turbofish type into a parse plan. `building` guards against
    /// recursive struct definitions, which fall back to dynamic parsing.
    pub(super) fn json_plan(&self, ty: &syn::Type, building: &mut Vec<String>) -> JsonPlan {
        let syn::Type::Path(p) = ty else {
            return JsonPlan::Dynamic;
        };
        let Some(seg) = p.path.segments.last() else {
            return JsonPlan::Dynamic;
        };
        let name = seg.ident.to_string();
        let generic = |i: usize| -> Option<&syn::Type> {
            match &seg.arguments {
                syn::PathArguments::AngleBracketed(a) => {
                    a.args.iter().filter_map(|g| match g {
                        syn::GenericArgument::Type(t) => Some(t),
                        _ => None,
                    }).nth(i)
                }
                _ => None,
            }
        };
        match name.as_str() {
            "Vec" | "VecDeque" => match generic(0) {
                Some(inner) => JsonPlan::Vec(Box::new(self.json_plan(inner, building))),
                None => JsonPlan::Dynamic,
            },
            "Option" | "Box" | "Rc" | "Arc" => match generic(0) {
                Some(inner) => self.json_plan(inner, building),
                None => JsonPlan::Dynamic,
            },
            "HashMap" | "BTreeMap" => match generic(1) {
                Some(inner) => JsonPlan::Map(Box::new(self.json_plan(inner, building))),
                None => JsonPlan::Dynamic,
            },
            _ => {
                if building.iter().any(|b| b == &name) {
                    return JsonPlan::Dynamic;
                }
                let Some(shape) = self.struct_shape(&name) else {
                    return JsonPlan::Dynamic;
                };
                let Some(def) = self.structs().get(&name).cloned() else {
                    return JsonPlan::Dynamic;
                };
                building.push(name);
                let mut fields = Vec::with_capacity(shape.runtime.fields.len());
                if let syn::Fields::Named(named) = &def.fields {
                    for f in &named.named {
                        if f.ident.is_none() {
                            continue;
                        }
                        fields.push(self.json_plan(&f.ty, building));
                    }
                }
                building.pop();
                JsonPlan::Struct(Rc::new(StructPlan { shape: shape.runtime.clone(), fields }))
            }
        }
    }

    /// `serde_json::from_str::<T>` with a known target type. Parses straight
    /// into typed values, so no coercion pass runs afterwards.
    pub(super) fn typed_from_str(&self, args: &[Value], ty: &syn::Type) -> Result<Value> {
        let owned;
        let text: &str = match args.first() {
            Some(Value::Str(s)) => s,
            Some(other) => {
                owned = other.display();
                &owned
            }
            None => bail!("from_str needs a string"),
        };
        let plan = self.json_plan(ty, &mut Vec::new());
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
        d.deserialize_any(PlanVisitor { plan: self.plan, keys: self.keys })
    }
}

/// Key seed that resolves an object key to its slot in the target struct,
/// without allocating. Unknown keys come back as None and are skipped.
struct FieldSeed<'a> {
    shape: &'a StructShape,
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
        Ok(self.shape.slot(s))
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
        while let Some(v) = seq.next_element_seed(PlanSeed { plan: elem, keys: self.keys })? {
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
                while let Some(slot) =
                    access.next_key_seed(FieldSeed { shape: &sp.shape })?
                {
                    match slot {
                        Some(i) => {
                            values[i] = access
                                .next_value_seed(PlanSeed { plan: &sp.fields[i], keys: self.keys })?;
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
                        access.next_value_seed(PlanSeed { plan: elem, keys: self.keys })?,
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
        Value::Int(i) => J::Number(serde_json::Number::from(*i as i64)),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(J::Number)
            .unwrap_or(J::Null),
        Value::Char(c) => J::String(c.to_string()),
        Value::Str(s) => J::String(s.to_string()),
        Value::Vec(items) | Value::Tuple(items) => {
            J::Array(items.borrow().iter().map(value_to_json).collect::<Result<_>>()?)
        }
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
            for (k, val) in s.shape.fields.iter().zip(values.iter()) {
                obj.insert(k.to_string(), value_to_json(val)?);
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
        Value::Native(n) => bail!("cannot serialize a {} to json", n.borrow().type_name()),
    })
}
