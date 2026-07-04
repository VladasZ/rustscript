//! Direct serde based json parsing plus conversions between script
//! values and `serde_json::Value`. Split from `builtins.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Result, bail};

use rustc_hash::FxHashMap;

use super::value::{MapKey, RStr, Value, map_with_capacity};



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

pub(super) fn bridge_serde_json(func: &str, args: &[Value]) -> Result<Value> {
    match func {
        "from_str" => {
            let s = match args.first() {
                Some(Value::Str(s)) => s.to_string(),
                Some(other) => other.display(),
                None => bail!("from_str needs a string"),
            };
            match parse_json(&s) {
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
        Value::Struct { fields, .. } => {
            let mut obj = serde_json::Map::default();
            for (k, val) in fields.borrow().iter() {
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
