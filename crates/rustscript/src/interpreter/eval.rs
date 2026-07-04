//! Value helpers shared by the VM and the bridges. Coercion into annotated
//! types, indexing, field access, the `?` operator, casts, and iteration. These
//! carry no control flow, the VM drives that.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use anyhow::{Result, anyhow, bail};

use super::value::{Fields, MapKey, Value};
use super::Interp;

impl Interp {
    /// Turn a dynamic value into `ty` when `ty` names a known struct, walking
    /// `Vec<T>`, `Option<T>`, and smart pointers. Anything else is unchanged.
    pub(super) fn coerce_value(&self, value: Value, ty: &syn::Type) -> Value {
        let syn::Type::Path(p) = ty else {
            return value;
        };
        let Some(seg) = p.path.segments.last() else {
            return value;
        };
        let name = seg.ident.to_string();
        match name.as_str() {
            "Vec" | "VecDeque" => {
                if let (Value::Vec(items), Some(inner)) = (&value, first_generic_type(seg)) {
                    let out = items
                        .borrow()
                        .iter()
                        .map(|v| self.coerce_value(v.clone(), inner))
                        .collect();
                    return Value::vec(out);
                }
                value
            }
            "Option" => {
                if let (Value::Enum { enum_name, variant, data }, Some(inner)) =
                    (&value, first_generic_type(seg))
                    && enum_name == "Option"
                    && variant == "Some"
                {
                    let coerced =
                        self.coerce_value(data.borrow().first().cloned().unwrap_or(Value::Unit), inner);
                    return Value::some(coerced);
                }
                value
            }
            "Box" | "Rc" | "Arc" => match first_generic_type(seg) {
                Some(inner) => self.coerce_value(value, inner),
                None => value,
            },
            _ => {
                if let Some(def) = self.structs().get(&name).cloned()
                    && let Value::Map(map) = &value
                {
                    return self.struct_from_map(&name, &def, &map.borrow());
                }
                value
            }
        }
    }

    /// If `value` is `Ok(x)` coerce `x`, otherwise coerce `value` directly.
    pub(super) fn coerce_result(&self, value: Value, ty: &syn::Type) -> Value {
        if let Value::Enum { enum_name, variant, data } = &value
            && enum_name == "Result"
            && variant == "Ok"
        {
            let inner = data.borrow().first().cloned().unwrap_or(Value::Unit);
            return Value::ok(self.coerce_value(inner, ty));
        }
        self.coerce_value(value, ty)
    }

    fn struct_from_map(
        &self,
        name: &str,
        def: &syn::ItemStruct,
        map: &BTreeMap<MapKey, Value>,
    ) -> Value {
        let mut fields = Fields::new();
        if let syn::Fields::Named(named) = &def.fields {
            for f in &named.named {
                let Some(ident) = &f.ident else { continue };
                let fname = ident.to_string();
                let raw = map
                    .get(&MapKey::Str(fname.clone()))
                    .cloned()
                    .unwrap_or_else(Value::none);
                fields.insert(fname, self.coerce_value(raw, &f.ty));
            }
        }
        Value::Struct { name: name.to_string(), fields: Rc::new(RefCell::new(fields)) }
    }

    /// Expand any iterable into a concrete list of items.
    pub(super) fn into_iter_items(&self, v: Value) -> Result<Vec<Value>> {
        Ok(match v {
            Value::Vec(items) => items.borrow().clone(),
            Value::Tuple(items) => items.borrow().clone(),
            Value::Range { start, end, inclusive } => {
                let end = if inclusive { end + 1 } else { end };
                (start..end).map(Value::Int).collect()
            }
            Value::Map(map) => map
                .borrow()
                .iter()
                .map(|(k, v)| Value::Tuple(Rc::new(RefCell::new(vec![k.to_value(), v.clone()]))))
                .collect(),
            Value::Str(s) => s.borrow().chars().map(Value::Char).collect(),
            Value::Native(h) if matches!(&*h.borrow(), super::native::Native::Lines(_)) => {
                super::native::drain_lines(&h)
            }
            other => bail!("{} is not iterable", other.type_name()),
        })
    }

    pub(super) fn eval_try(&self, v: Value) -> Result<std::result::Result<Value, Value>> {
        match v {
            Value::Enum { enum_name, variant, data } if enum_name == "Result" => {
                let inner = data.borrow().first().cloned().unwrap_or(Value::Unit);
                if variant == "Ok" {
                    Ok(Ok(inner))
                } else {
                    Ok(Err(Value::err(inner)))
                }
            }
            Value::Enum { enum_name, variant, data } if enum_name == "Option" => {
                let inner = data.borrow().first().cloned().unwrap_or(Value::Unit);
                if variant == "Some" {
                    Ok(Ok(inner))
                } else {
                    Ok(Err(Value::none()))
                }
            }
            other => bail!("the `?` operator needs a Result or Option, got {}", other.type_name()),
        }
    }

    pub(super) fn eval_cast(&self, v: Value, ty: &syn::Type) -> Result<Value> {
        let target = match ty {
            syn::Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
            _ => None,
        };
        let target = target.unwrap_or_default();
        Ok(match target.as_str() {
            "f64" | "f32" => Value::Float(match v {
                Value::Int(i) => i as f64,
                Value::Float(f) => f,
                other => bail!("cannot cast {} to float", other.type_name()),
            }),
            "usize" | "u8" | "u16" | "u32" | "u64" | "u128" | "isize" | "i8" | "i16" | "i32"
            | "i64" | "i128" => Value::Int(match v {
                Value::Int(i) => i,
                Value::Float(f) => f as i64,
                Value::Char(c) => c as i64,
                Value::Bool(b) => b as i64,
                other => bail!("cannot cast {} to integer", other.type_name()),
            }),
            "char" => match v {
                Value::Int(i) => Value::Char(
                    char::from_u32(i as u32).ok_or_else(|| anyhow!("invalid char code {i}"))?,
                ),
                Value::Char(c) => Value::Char(c),
                other => bail!("cannot cast {} to char", other.type_name()),
            },
            other => bail!("unsupported cast target: {other}"),
        })
    }

    // -- indexing and fields ----------------------------------------------

    pub(super) fn index(&self, base: &Value, key: &Value) -> Result<Value> {
        Ok(match base {
            Value::Vec(items) => {
                let i = as_index(key)?;
                items.borrow().get(i).cloned().ok_or_else(|| anyhow!("index {i} out of bounds"))?
            }
            Value::Str(s) => {
                let i = as_index(key)?;
                s.borrow()
                    .chars()
                    .nth(i)
                    .map(Value::Char)
                    .ok_or_else(|| anyhow!("index {i} out of bounds"))?
            }
            Value::Tuple(items) => {
                let i = as_index(key)?;
                items.borrow().get(i).cloned().ok_or_else(|| anyhow!("index {i} out of bounds"))?
            }
            Value::Map(map) => {
                let k = key.as_key().ok_or_else(|| anyhow!("{} is not a valid map key", key.type_name()))?;
                map.borrow().get(&k).cloned().ok_or_else(|| anyhow!("key not found"))?
            }
            Value::Struct { name, fields } if name == "Captures" => {
                super::builtins::capture_index(fields, key)?
            }
            other => bail!("cannot index {}", other.type_name()),
        })
    }

    pub(super) fn set_index(&self, base: &Value, key: &Value, val: Value) -> Result<()> {
        match base {
            Value::Vec(items) => {
                let i = as_index(key)?;
                let mut items = items.borrow_mut();
                if i >= items.len() {
                    bail!("index {i} out of bounds (len {})", items.len());
                }
                items[i] = val;
            }
            Value::Map(map) => {
                let k = key.as_key().ok_or_else(|| anyhow!("{} is not a valid map key", key.type_name()))?;
                map.borrow_mut().insert(k, val);
            }
            other => bail!("cannot index-assign into {}", other.type_name()),
        }
        Ok(())
    }

    pub(super) fn get_field(&self, base: &Value, member: &super::bytecode::Member) -> Result<Value> {
        use super::bytecode::Member;
        match (base, member) {
            (Value::Struct { fields, .. }, Member::Named(name)) => fields
                .borrow()
                .get(name)
                .cloned()
                .ok_or_else(|| anyhow!("no field `{name}`")),
            (Value::Tuple(items), Member::Indexed(i)) => items
                .borrow()
                .get(*i)
                .cloned()
                .ok_or_else(|| anyhow!("no field `{i}`")),
            (Value::Struct { fields, .. }, Member::Indexed(i)) => fields
                .borrow()
                .get(&i.to_string())
                .cloned()
                .ok_or_else(|| anyhow!("no field `{i}`")),
            (b, _) => bail!("cannot access field of {}", b.type_name()),
        }
    }

    pub(super) fn set_field(&self, base: &Value, member: &super::bytecode::Member, val: Value) -> Result<()> {
        use super::bytecode::Member;
        match (base, member) {
            (Value::Struct { fields, .. }, Member::Named(name)) => {
                fields.borrow_mut().insert(name.clone(), val);
            }
            (Value::Struct { fields, .. }, Member::Indexed(i)) => {
                fields.borrow_mut().insert(i.to_string(), val);
            }
            (Value::Tuple(items), Member::Indexed(i)) => {
                items.borrow_mut()[*i] = val;
            }
            (b, _) => bail!("cannot assign to field of {}", b.type_name()),
        }
        Ok(())
    }
}

fn as_index(key: &Value) -> Result<usize> {
    match key {
        Value::Int(i) if *i >= 0 => Ok(*i as usize),
        Value::Int(i) => bail!("negative index {i}"),
        other => bail!("index must be an integer, got {}", other.type_name()),
    }
}

/// First concrete type argument of a path segment, `Vec<T>` gives `T`.
pub(super) fn first_generic_type(seg: &syn::PathSegment) -> Option<&syn::Type> {
    if let syn::PathArguments::AngleBracketed(ab) = &seg.arguments {
        for a in &ab.args {
            if let syn::GenericArgument::Type(t) = a {
                return Some(t);
            }
        }
    }
    None
}
