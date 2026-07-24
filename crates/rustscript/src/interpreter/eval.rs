//! Value helpers shared by the VM and the bridges. Coercion into annotated
//! types, indexing, field access, the `?` operator, casts, and iteration. These
//! carry no control flow, the VM drives that.

use std::rc::Rc;

use anyhow::{Result, anyhow, bail};

use super::Interp;
use super::numeric::{IntWidth, float_to_int, truncate};
use super::resolver::Res;
use super::value::{KeyRef, Map, StructShape, Value};

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

/// The exact message a compiled binary panics with on a bad index.
fn oob(len: usize, i: usize) -> anyhow::Error {
    anyhow!("index out of bounds: the len is {len} but the index is {i}")
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
                                        Value::Map(m) => self.struct_from_map(&shape, &m.borrow()),
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
                if let (
                    Value::Enum {
                        enum_name,
                        variant,
                        data,
                    },
                    Some(inner),
                ) = (&value, first_generic_type(seg))
                    && &**enum_name == "Option"
                    && &**variant == "Some"
                {
                    let coerced = self.coerce_value(
                        data.first().cloned().unwrap_or(Value::Unit),
                        inner,
                        module,
                    );
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
        if let Value::Enum {
            enum_name,
            variant,
            data,
        } = &value
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
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
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
        let mut renames: Vec<Option<Rc<str>>> = Vec::new();
        let mut coerce = Vec::new();
        if let syn::Fields::Named(named) = &def.fields {
            for f in &named.named {
                let Some(ident) = &f.ident else { continue };
                fields.push(ident.to_string().into());
                renames.push(super::json_bridge::serde_rename(f).map(Rc::from));
                coerce.push(self.field_needs_coerce(&f.ty, module).then(|| f.ty.clone()));
            }
        }
        let shape = Rc::new(Shape {
            runtime: StructShape::with_renames(canon.clone(), fields, renames),
            coerce,
            module,
        });
        self.shapes
            .borrow_mut()
            .insert(canon.clone(), shape.clone());
        Some(shape)
    }

    /// Whether coercing a value into `ty` can do anything. Containers, known
    /// struct names, and aliases to them can, primitives cannot.
    fn field_needs_coerce(&self, ty: &syn::Type, module: usize) -> bool {
        let syn::Type::Path(p) = ty else { return false };
        let Some(seg) = p.path.segments.last() else {
            return false;
        };
        let name = seg.ident.to_string();
        matches!(
            name.as_str(),
            "Vec" | "VecDeque" | "Option" | "Box" | "Rc" | "Arc"
        ) || self
            .resolver()
            .resolve_struct_key(module, &p.path)
            .is_some()
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

    pub(super) fn eval_try(&self, v: Value) -> Result<std::result::Result<Value, Value>> {
        match v {
            Value::Enum {
                enum_name,
                variant,
                data,
            } if &*enum_name == "Result" => {
                let inner = data.first().cloned().unwrap_or(Value::Unit);
                if &*variant == "Ok" {
                    Ok(Ok(inner))
                } else {
                    Ok(Err(Value::err(inner)))
                }
            }
            Value::Enum {
                enum_name,
                variant,
                data,
            } if &*enum_name == "Option" => {
                let inner = data.first().cloned().unwrap_or(Value::Unit);
                if &*variant == "Some" {
                    Ok(Ok(inner))
                } else {
                    Ok(Err(Value::none()))
                }
            }
            // A json accessor hands its value back already unwrapped, a json
            // string is a plain String here. Let `?` pass it through as its
            // own Some, the rule match, if let, and or_else already follow,
            // see the json_option example. Scripts pass a real cargo check,
            // so `?` never reaches a value that is not Option or Result in
            // the source types.
            other => Ok(Ok(other)),
        }
    }

    /// An `as` cast to a named primitive type, with real Rust semantics per
    /// width: integer casts truncate, float to integer casts saturate, and
    /// f32 becomes a real f32 value.
    pub(super) fn eval_cast(&self, v: Value, ty: &syn::Type) -> Result<Value> {
        let target = match ty {
            syn::Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
            _ => None,
        };
        let target = target.unwrap_or_default();
        let target = target.as_str();
        if target == "f64" {
            return Ok(Value::Float(match v {
                Value::Int(i) => i as f64,
                Value::IntW(..) => v.int_parts().unwrap().0 as f64,
                Value::Float(f) => f,
                Value::F32(f) => f64::from(f),
                other => bail!("cannot cast {} to float", other.type_name()),
            }));
        }
        if target == "f32" {
            return Ok(Value::F32(match v {
                Value::Int(i) => i as f32,
                Value::IntW(..) => v.int_parts().unwrap().0 as f32,
                Value::Float(f) => f as f32,
                Value::F32(f) => f,
                other => bail!("cannot cast {} to float", other.type_name()),
            }));
        }
        if target == "char" {
            return Ok(match v {
                Value::Int(i) => Value::Char(
                    char::from_u32(i as u32).ok_or_else(|| anyhow!("invalid char code {i}"))?,
                ),
                Value::Char(c) => Value::Char(c),
                other => bail!("cannot cast {} to char", other.type_name()),
            });
        }
        // u128 and i128 carry no runtime width yet and keep the old
        // i64-passthrough.
        let width = match target {
            "u128" | "i128" => IntWidth::I64,
            _ => IntWidth::parse(target)
                .ok_or_else(|| anyhow!("unsupported cast target: {target}"))?,
        };
        let value = match v {
            Value::Int(i) => truncate(i128::from(i), width),
            Value::IntW(..) => truncate(v.int_parts().unwrap().0, width),
            Value::Float(f) => float_to_int(f, width),
            // The f64 image of an f32 is the same real number, so saturating
            // through it is exact.
            Value::F32(f) => float_to_int(f64::from(f), width),
            Value::Char(c) => truncate(i128::from(c as u32), width),
            Value::Bool(b) => i128::from(b),
            other => bail!("cannot cast {} to integer", other.type_name()),
        };
        Ok(Value::int_of_width(value, width))
    }

    // -- indexing and fields ----------------------------------------------

    pub(super) fn index(&self, base: &Value, key: &Value) -> Result<Value> {
        if let Value::Range {
            start,
            end,
            inclusive,
        } = key
        {
            return slice_value(base, *start, *end, *inclusive);
        }
        Ok(match base {
            Value::Vec(items) => {
                let i = as_index(key)?;
                let items = items.borrow();
                items.get(i).cloned().ok_or_else(|| oob(items.len(), i))?
            }
            Value::Str(s) => {
                let i = as_index(key)?;
                s.chars()
                    .nth(i)
                    .map(Value::Char)
                    .ok_or_else(|| oob(s.chars().count(), i))?
            }
            Value::Tuple(items) => {
                let i = as_index(key)?;
                let items = items.borrow();
                items.get(i).cloned().ok_or_else(|| oob(items.len(), i))?
            }
            Value::Map(map) => {
                let k = key
                    .key_ref()
                    .ok_or_else(|| anyhow!("{} is not a valid map key", key.type_name()))?;
                map.borrow()
                    .get(&k)
                    .cloned()
                    .ok_or_else(|| anyhow!("no entry found for key"))?
            }
            Value::Native(handle)
                if matches!(&*handle.borrow(), super::native::Native::RegexCaptures(_)) =>
            {
                super::regex_bridge::capture_index(handle, key)?
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
                    return Err(oob(items.len(), i));
                }
                items[i] = val;
            }
            Value::Map(map) => {
                let k = key
                    .as_key()
                    .ok_or_else(|| anyhow!("{} is not a valid map key", key.type_name()))?;
                map.borrow_mut().insert(k, val);
            }
            other => bail!("cannot index-assign into {}", other.type_name()),
        }
        Ok(())
    }

    pub(super) fn get_field(
        &self,
        base: &Value,
        member: &super::bytecode::Member,
    ) -> Result<Value> {
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

    pub(super) fn set_field(
        &self,
        base: &Value,
        member: &super::bytecode::Member,
        val: Value,
    ) -> Result<()> {
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
        Value::IntW(..) => {
            let (v, _) = key.int_parts().unwrap();
            usize::try_from(v).map_err(|_| anyhow!("index {v} out of range"))
        }
        other => bail!("index must be an integer, got {}", other.type_name()),
    }
}

/// `v[a..b]` on vectors and byte-based `s[a..b]` on strings, matching real
/// slice semantics. An i64::MAX end is the open-end sentinel meaning len.
fn slice_value(base: &Value, start: i64, end: i64, inclusive: bool) -> Result<Value> {
    let bounds = |len: usize| -> Result<(usize, usize)> {
        if start < 0 {
            bail!("negative slice start {start}");
        }
        let end = if end == i64::MAX {
            len as i64
        } else if inclusive {
            end + 1
        } else {
            end
        };
        if end < start || end as usize > len {
            bail!("slice {start}..{end} out of bounds (len {len})");
        }
        Ok((start as usize, end as usize))
    };
    match base {
        Value::Vec(items) => {
            let items = items.borrow();
            let (a, b) = bounds(items.len())?;
            Ok(Value::vec(items[a..b].to_vec()))
        }
        Value::Str(s) => {
            let (a, b) = bounds(s.len())?;
            match s.get(a..b) {
                Some(sub) => Ok(Value::str(sub.to_string())),
                None => bail!("slice {a}..{b} is not on a char boundary"),
            }
        }
        other => bail!("cannot slice {}", other.type_name()),
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
