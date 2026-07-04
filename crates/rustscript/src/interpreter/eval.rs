//! Value helpers shared by the VM and the bridges. Coercion into annotated
//! types, indexing, field access, the `?` operator, casts, and iteration. These
//! carry no control flow, the VM drives that.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Result, anyhow, bail};

use super::resolver::Res;
use super::value::{KeyRef, Map, StructShape, Value};
use super::Interp;

/// Field layout of a user struct, built once per struct and reused for every
/// coerced instance so field names are shared instead of re-allocated.
pub(super) struct Shape {
    /// Runtime layout shared by every instance built from this shape.
    pub runtime: Rc<StructShape>,
    /// Per field, its type when coercion can change the value. None means
    /// the type is a primitive and coercion is a no-op, skipped per instance.
    pub coerce: Vec<Option<syn::Type>>,
    /// Module the struct is declared in. Field types resolve against it.
    pub module: usize,
}

impl Interp {
    /// Turn a dynamic value into `ty` when `ty` names a known struct, walking
    /// `Vec<T>`, `Option<T>`, smart pointers, and type aliases. `module` is
    /// where the type annotation was written. Anything else is unchanged.
    pub(super) fn coerce_value(&self, value: Value, ty: &syn::Type, module: usize) -> Value {
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
                    // A struct element type resolves once for the whole vector,
                    // and a primitive element type needs no work at all.
                    if let syn::Type::Path(ip) = inner
                        && let Some(iseg) = ip.path.segments.last()
                        && !matches!(iseg.arguments, syn::PathArguments::AngleBracketed(_))
                    {
                        return match self.shape_for(module, &ip.path) {
                            Some(shape) => Value::vec(
                                items
                                    .borrow()
                                    .iter()
                                    .map(|v| match v {
                                        Value::Map(m) => {
                                            self.struct_from_map(&shape, &m.borrow())
                                        }
                                        other => other.clone(),
                                    })
                                    .collect(),
                            ),
                            None => value,
                        };
                    }
                    let out = items
                        .borrow()
                        .iter()
                        .map(|v| self.coerce_value(v.clone(), inner, module))
                        .collect();
                    return Value::vec(out);
                }
                value
            }
            "Option" => {
                if let (Value::Enum { enum_name, variant, data }, Some(inner)) =
                    (&value, first_generic_type(seg))
                    && &**enum_name == "Option"
                    && &**variant == "Some"
                {
                    let coerced = self
                        .coerce_value(data.first().cloned().unwrap_or(Value::Unit), inner, module);
                    return Value::some(coerced);
                }
                value
            }
            "Box" | "Rc" | "Arc" => match first_generic_type(seg) {
                Some(inner) => self.coerce_value(value, inner, module),
                None => value,
            },
            _ => {
                if let Some(shape) = self.shape_for(module, &p.path)
                    && let Value::Map(map) = &value
                {
                    return self.struct_from_map(&shape, &map.borrow());
                }
                if let Some((am, target)) = self.follow_alias(module, &p.path) {
                    return self.coerce_value(value, &target, am);
                }
                value
            }
        }
    }

    /// If `value` is `Ok(x)` coerce `x`, otherwise coerce `value` directly.
    pub(super) fn coerce_result(&self, value: Value, ty: &syn::Type, module: usize) -> Value {
        if let Value::Enum { enum_name, variant, data } = &value
            && &**enum_name == "Result"
            && &**variant == "Ok"
        {
            let inner = data.first().cloned().unwrap_or(Value::Unit);
            return Value::ok(self.coerce_value(inner, ty, module));
        }
        self.coerce_value(value, ty, module)
    }

    /// Field layout of the struct a type path names in `module`, if any.
    pub(super) fn shape_for(&self, module: usize, path: &syn::Path) -> Option<Rc<Shape>> {
        let canon = self.resolver().resolve_struct_key(module, path)?;
        self.struct_shape(&canon)
    }

    /// A type alias hit by this path, with the module its target resolves in.
    fn follow_alias(&self, module: usize, path: &syn::Path) -> Option<(usize, Rc<syn::Type>)> {
        let segs: Vec<String> =
            path.segments.iter().map(|s| s.ident.to_string()).collect();
        match self.resolver().resolve(module, &segs) {
            Ok(Res::Alias(m, target)) => Some((m, target)),
            _ => None,
        }
    }

    /// Cached field layout for a known struct, by canonical name.
    pub(super) fn struct_shape(&self, canon: &Rc<str>) -> Option<Rc<Shape>> {
        if let Some(shape) = self.shapes.borrow().get(canon) {
            return Some(shape.clone());
        }
        let def = self.structs().get(canon)?;
        let module = def.module;
        let def = def.ast.clone();
        let mut fields: Vec<Rc<str>> = Vec::new();
        let mut coerce = Vec::new();
        if let syn::Fields::Named(named) = &def.fields {
            for f in &named.named {
                let Some(ident) = &f.ident else { continue };
                fields.push(ident.to_string().into());
                coerce.push(self.field_needs_coerce(&f.ty, module).then(|| f.ty.clone()));
            }
        }
        let shape = Rc::new(Shape {
            runtime: StructShape::new(canon.clone(), fields),
            coerce,
            module,
        });
        self.shapes.borrow_mut().insert(canon.clone(), shape.clone());
        Some(shape)
    }

    /// Whether coercing a value into `ty` can do anything. Containers, known
    /// struct names, and aliases to them can, primitives cannot.
    fn field_needs_coerce(&self, ty: &syn::Type, module: usize) -> bool {
        let syn::Type::Path(p) = ty else { return false };
        let Some(seg) = p.path.segments.last() else { return false };
        let name = seg.ident.to_string();
        matches!(name.as_str(), "Vec" | "VecDeque" | "Option" | "Box" | "Rc" | "Arc")
            || self.resolver().resolve_struct_key(module, &p.path).is_some()
            || self.follow_alias(module, &p.path).is_some()
    }

    pub(super) fn struct_from_map(&self, shape: &Shape, map: &Map) -> Value {
        let mut values = Vec::with_capacity(shape.coerce.len());
        for (fname, ty) in shape.runtime.fields.iter().zip(&shape.coerce) {
            let raw = map
                .get(&KeyRef::Str(fname))
                .cloned()
                .unwrap_or_else(Value::none);
            let coerced = match ty {
                Some(t) => self.coerce_value(raw, t, shape.module),
                None => raw,
            };
            values.push(coerced);
        }
        Value::structure(shape.runtime.clone(), values)
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
            Value::Str(s) => s.chars().map(Value::Char).collect(),
            Value::Native(h) if matches!(&*h.borrow(), super::native::Native::Lines(_)) => {
                super::native::drain_lines(&h)
            }
            other => bail!("{} is not iterable", other.type_name()),
        })
    }

    pub(super) fn eval_try(&self, v: Value) -> Result<std::result::Result<Value, Value>> {
        match v {
            Value::Enum { enum_name, variant, data } if &*enum_name == "Result" => {
                let inner = data.first().cloned().unwrap_or(Value::Unit);
                if &*variant == "Ok" {
                    Ok(Ok(inner))
                } else {
                    Ok(Err(Value::err(inner)))
                }
            }
            Value::Enum { enum_name, variant, data } if &*enum_name == "Option" => {
                let inner = data.first().cloned().unwrap_or(Value::Unit);
                if &*variant == "Some" {
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
                s.chars()
                    .nth(i)
                    .map(Value::Char)
                    .ok_or_else(|| anyhow!("index {i} out of bounds"))?
            }
            Value::Tuple(items) => {
                let i = as_index(key)?;
                items.borrow().get(i).cloned().ok_or_else(|| anyhow!("index {i} out of bounds"))?
            }
            Value::Map(map) => {
                let k = key
                    .key_ref()
                    .ok_or_else(|| anyhow!("{} is not a valid map key", key.type_name()))?;
                map.borrow().get(&k).cloned().ok_or_else(|| anyhow!("key not found"))?
            }
            Value::Struct(s) if &**s.name() == "Captures" => {
                super::regex_bridge::capture_index(s, key)?
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
            (Value::Struct(s), Member::Named(name)) => {
                s.get(name).ok_or_else(|| anyhow!("no field `{name}`"))
            }
            (Value::Tuple(items), Member::Indexed(i)) => items
                .borrow()
                .get(*i)
                .cloned()
                .ok_or_else(|| anyhow!("no field `{i}`")),
            (Value::Struct(s), Member::Indexed(i)) => s
                .values
                .borrow()
                .get(*i)
                .cloned()
                .ok_or_else(|| anyhow!("no field `{i}`")),
            (b, _) => bail!("cannot access field of {}", b.type_name()),
        }
    }

    pub(super) fn set_field(&self, base: &Value, member: &super::bytecode::Member, val: Value) -> Result<()> {
        use super::bytecode::Member;
        match (base, member) {
            (Value::Struct(s), Member::Named(name)) => {
                if !s.set(name, val) {
                    bail!("no field `{name}`");
                }
            }
            (Value::Struct(s), Member::Indexed(i)) => {
                let mut values = s.values.borrow_mut();
                match values.get_mut(*i) {
                    Some(slot) => *slot = val,
                    None => bail!("no field `{i}`"),
                }
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
