//! The serde layout of script structs and enums, precomputed at load, and how a script enum
//! reads from and writes to a json value. An enum reads from the parsed tree, not the token
//! stream, because an internally tagged or untagged enum must see the whole object first.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, bail};
use rustc_hash::FxHashMap;
use serde_json::Value as JsonValue;

use super::Interp;
use super::bytecode::PathId;
use super::enum_def::{EnumDef, SerdeRepr};
use super::json_bridge::{JsonPlan, ParseCx, PlanVisitor, StructInfo, Structs};
use super::typeir::{TypeIr, lower_type};
use super::value::{MapKey, MapStore, StructShape, Value};
use super::vm::Vm;

pub struct EnumInfo {
    pub def: Arc<EnumDef>,
    pub variants: Vec<VariantPayload>,
}

pub enum VariantPayload {
    Unit,
    Newtype(TypeIr, Scalar),
    Tuple(Vec<(TypeIr, Scalar)>),
    Struct(Arc<StructInfo>),
}

/// What json a scalar payload accepts, so an untagged enum can tell `I(i64)` from `S(String)`.
#[derive(Clone, Copy, PartialEq)]
pub enum Scalar {
    Int,
    Float,
    Str,
    Bool,
    Any,
}

pub type Enums = HashMap<Arc<str>, Arc<EnumInfo>>;

/// A json value that does not fit the target type. `from_str` turns it into an `Err`, not a
/// panic.
#[derive(Debug)]
pub struct DataError(pub String);

impl std::fmt::Display for DataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DataError {}

impl Interp {
    pub(super) fn build_structs(&self) -> Structs {
        let mut out = Structs::default();
        for (canon, def) in self.structs() {
            let rule = super::serde_attrs::serde_rename_all(&def.ast.attrs);
            let shape_name: Arc<str> = Arc::from(&**canon);
            let info =
                self.struct_info(&shape_name, &def.ast.fields, rule, def.module, Some(canon));
            out.insert(shape_name, Arc::new(info));
        }
        out
    }

    /// `defaults_of` names the struct whose `#[serde(default)]` chunks apply, a struct variant
    /// has none.
    fn struct_info(
        &self,
        shape_name: &Arc<str>,
        fields_ast: &syn::Fields,
        rule: Option<super::serde_attrs::RenameRule>,
        module: usize,
        defaults_of: Option<&Arc<str>>,
    ) -> StructInfo {
        let mut fields: Vec<Arc<str>> = Vec::new();
        let mut renames: Vec<Option<Arc<str>>> = Vec::new();
        let mut skip_none: Vec<bool> = Vec::new();
        let mut coerce = Vec::new();
        let mut json = Vec::new();
        let mut optional = Vec::new();
        let mut defaults = Vec::new();
        let mut key_map = FxHashMap::default();
        if let syn::Fields::Named(named) = fields_ast {
            let mut slot = 0;
            for f in &named.named {
                let Some(ident) = &f.ident else { continue };
                let name = ident.to_string();
                let rename =
                    super::serde_attrs::serde_rename(f).or_else(|| rule.map(|r| r.apply(&name)));
                fields.push(Arc::from(name.as_str()));
                renames.push(rename.as_deref().map(Arc::from));
                skip_none.push(super::serde_attrs::serde_skip_none(f));
                let ir = lower_type(&f.ty, self.resolver(), module, &[]);
                coerce.push(ir.is_active().then(|| ir.clone()));
                json.push(ir);
                optional.push(is_option(&f.ty));
                key_map.insert(rename.unwrap_or(name), slot);
                defaults.push(
                    defaults_of.and_then(|c| self.serde_defaults.get(&(c.clone(), slot)).cloned()),
                );
                slot += 1;
            }
        }
        let shape = StructShape::typed(
            shape_name.clone(),
            self.resolver().type_id_of(shape_name),
            fields,
            renames,
            skip_none,
        );
        StructInfo {
            shape,
            coerce,
            json,
            optional,
            key_map,
            defaults,
        }
    }

    pub(super) fn build_enums(&self) -> Enums {
        let mut out = Enums::default();
        for (canon, def) in &self.resolver().enum_defs {
            let Some(ast) = self.resolver().enums.get(canon) else {
                continue;
            };
            let module = self.enum_module(canon);
            let mut variants = Vec::new();
            for (index, v) in ast.variants.iter().enumerate() {
                let payload = match &v.fields {
                    syn::Fields::Unit => VariantPayload::Unit,
                    syn::Fields::Unnamed(u) if u.unnamed.len() == 1 => {
                        let ty = &u.unnamed[0].ty;
                        VariantPayload::Newtype(
                            lower_type(ty, self.resolver(), module, &[]),
                            scalar_of(ty),
                        )
                    }
                    syn::Fields::Unnamed(u) => VariantPayload::Tuple(
                        u.unnamed
                            .iter()
                            .map(|f| {
                                (
                                    lower_type(&f.ty, self.resolver(), module, &[]),
                                    scalar_of(&f.ty),
                                )
                            })
                            .collect(),
                    ),
                    syn::Fields::Named(_) => {
                        let name: Arc<str> = Arc::from(v.ident.to_string().as_str());
                        let mut info = self.struct_info(&name, &v.fields, None, module, None);
                        let index = u16::try_from(index).unwrap_or(u16::MAX);
                        info.shape = info.shape.as_variant(def.clone(), index);
                        VariantPayload::Struct(Arc::new(info))
                    }
                };
                variants.push(payload);
            }
            out.insert(
                canon.clone(),
                Arc::new(EnumInfo {
                    def: def.clone(),
                    variants,
                }),
            );
        }
        out
    }

    /// The module an enum is declared in, which its payload types resolve against.
    fn enum_module(&self, canon: &str) -> usize {
        self.resolver()
            .modules
            .iter()
            .position(|m| m.enums.values().any(|c| &**c == canon))
            .unwrap_or(0)
    }
}

fn is_option(ty: &syn::Type) -> bool {
    matches!(ty, syn::Type::Path(p)
        if p.path.segments.last().is_some_and(|s| s.ident == "Option"))
}

fn scalar_of(ty: &syn::Type) -> Scalar {
    let syn::Type::Path(p) = ty else {
        return Scalar::Any;
    };
    let Some(last) = p.path.segments.last() else {
        return Scalar::Any;
    };
    let name = last.ident.to_string();
    if super::numeric::IntWidth::parse(&name).is_some() {
        return Scalar::Int;
    }
    match name.as_str() {
        "f32" | "f64" => Scalar::Float,
        "String" | "str" | "char" | "PathBuf" => Scalar::Str,
        "bool" => Scalar::Bool,
        _ => Scalar::Any,
    }
}

// reading

impl Vm {
    pub(super) fn enum_from_json(
        self: &Arc<Self>,
        info: &EnumInfo,
        v: Value,
    ) -> std::result::Result<Value, String> {
        let def = &info.def;
        match &def.serde.repr {
            SerdeRepr::External => match &v {
                Value::Str(name) => {
                    let index = variant_named(def, name)?;
                    match info.variants.get(usize::from(index)) {
                        Some(VariantPayload::Unit) => Ok(Value::enum_of(def, index, Vec::new())),
                        _ => Err(format!(
                            "invalid type: unit variant, expected {} variant",
                            payload_kind(info, index)
                        )),
                    }
                }
                Value::Map(m, _) if m.lock().len() == 1 => {
                    let (key, payload) = {
                        let store = m.lock();
                        let Some((k, p)) = store.first() else {
                            return Err(expected_enum(def));
                        };
                        (k.to_value().display(), p.clone())
                    };
                    let index = variant_named(def, &key)?;
                    self.variant_from_json(info, index, payload)
                }
                _ => Err(expected_enum(def)),
            },
            SerdeRepr::Internal(tag) => {
                let Value::Map(m, _) = &v else {
                    return Err(expected_enum(def));
                };
                let mut rest = m.lock().clone();
                let Some(name) = rest.shift_remove(&MapKey::Str((&**tag).into())) else {
                    return Err(format!("missing field `{tag}`"));
                };
                let index = variant_named(def, &name.display())?;
                match info.variants.get(usize::from(index)) {
                    Some(VariantPayload::Unit) => Ok(Value::enum_of(def, index, Vec::new())),
                    Some(VariantPayload::Struct(si)) => self.struct_checked(si, &rest),
                    Some(VariantPayload::Newtype(ty, scalar)) => {
                        let inner = self.from_json_checked(Value::map_of(rest), ty, *scalar)?;
                        Ok(Value::enum_of(def, index, vec![inner]))
                    }
                    _ => Err("invalid type: sequence, expected a tagged variant".to_string()),
                }
            }
            SerdeRepr::Adjacent(tag, content) => {
                let Value::Map(m, _) = &v else {
                    return Err(expected_enum(def));
                };
                let store = m.lock().clone();
                let Some(name) = store.get(&MapKey::Str((&**tag).into())) else {
                    return Err(format!("missing field `{tag}`"));
                };
                let index = variant_named(def, &name.display())?;
                match store.get(&MapKey::Str((&**content).into())) {
                    Some(payload) => self.variant_from_json(info, index, payload.clone()),
                    None if matches!(
                        info.variants.get(usize::from(index)),
                        Some(VariantPayload::Unit)
                    ) =>
                    {
                        Ok(Value::enum_of(def, index, Vec::new()))
                    }
                    None => Err(format!("missing field `{content}`")),
                }
            }
            SerdeRepr::Untagged => {
                for index in 0..info.variants.len() {
                    let index = u16::try_from(index).unwrap_or(u16::MAX);
                    let found = match info.variants.get(usize::from(index)) {
                        Some(VariantPayload::Unit) if v.is_none_value() => {
                            Ok(Value::enum_of(def, index, Vec::new()))
                        }
                        Some(VariantPayload::Unit) => continue,
                        _ => self.variant_from_json(info, index, v.clone()),
                    };
                    if let Ok(value) = found {
                        return Ok(value);
                    }
                }
                Err(format!(
                    "data did not match any variant of untagged enum {}",
                    short_name(def)
                ))
            }
        }
    }

    fn variant_from_json(
        self: &Arc<Self>,
        info: &EnumInfo,
        index: u16,
        payload: Value,
    ) -> std::result::Result<Value, String> {
        let def = &info.def;
        match info.variants.get(usize::from(index)) {
            Some(VariantPayload::Unit) if payload.is_none_value() => {
                Ok(Value::enum_of(def, index, Vec::new()))
            }
            Some(VariantPayload::Unit) => Err("invalid type, expected unit".to_string()),
            Some(VariantPayload::Newtype(ty, scalar)) => {
                let inner = self.from_json_checked(payload, ty, *scalar)?;
                Ok(Value::enum_of(def, index, vec![inner]))
            }
            Some(VariantPayload::Tuple(items)) => {
                let Value::Vec(list) = &payload else {
                    return Err(format!(
                        "invalid type, expected tuple variant {}::{}",
                        short_name(def),
                        def.variant_name(index)
                    ));
                };
                let list = list.lock().clone();
                if list.len() != items.len() {
                    return Err(format!(
                        "invalid length {}, expected tuple variant {}::{} with {} elements",
                        list.len(),
                        short_name(def),
                        def.variant_name(index),
                        items.len()
                    ));
                }
                let mut out = Vec::with_capacity(list.len());
                for (v, (ty, scalar)) in list.into_iter().zip(items) {
                    out.push(self.from_json_checked(v, ty, *scalar)?);
                }
                Ok(Value::enum_of(def, index, out))
            }
            Some(VariantPayload::Struct(si)) => match &payload {
                Value::Map(m, _) => {
                    let store = m.lock().clone();
                    self.struct_checked(si, &store)
                }
                _ => Err(format!(
                    "invalid type, expected struct variant {}::{}",
                    short_name(def),
                    def.variant_name(index)
                )),
            },
            None => Err(expected_enum(def)),
        }
    }

    /// A json value into a declared type with the checks serde makes, so an untagged enum can
    /// move on to its next variant.
    pub(super) fn from_json_checked(
        self: &Arc<Self>,
        v: Value,
        ty: &TypeIr,
        scalar: Scalar,
    ) -> std::result::Result<Value, String> {
        let fits = match scalar {
            Scalar::Any => true,
            Scalar::Int => matches!(v, Value::Int(_) | Value::IntW(..) | Value::Big(..)),
            Scalar::Float => matches!(
                v,
                Value::Int(_) | Value::IntW(..) | Value::Float(_) | Value::F32(_)
            ),
            Scalar::Str => matches!(v, Value::Str(_)),
            Scalar::Bool => matches!(v, Value::Bool(_)),
        };
        if !fits {
            return Err(format!("invalid type: {}", v.type_name()));
        }
        match ty {
            TypeIr::Struct(canon) => match (&v, self.structs.get(&**canon)) {
                (Value::Map(m, _), Some(si)) => {
                    let store = m.lock().clone();
                    self.struct_checked(si, &store)
                }
                (_, Some(_)) => Err(format!("invalid type: {}, expected struct", v.type_name())),
                (_, None) => Ok(v),
            },
            TypeIr::Enum(canon) => match self.serde_enums.get(&**canon) {
                Some(info) => {
                    let info = info.clone();
                    self.enum_from_json(&info, v)
                }
                None => Ok(v),
            },
            TypeIr::Option(inner) => {
                if v.is_none_value() {
                    return Ok(v);
                }
                Ok(Value::some(self.from_json_checked(
                    v,
                    inner,
                    Scalar::Any,
                )?))
            }
            TypeIr::Vec(inner) => {
                let Value::Vec(items) = &v else {
                    return Err(format!(
                        "invalid type: {}, expected a sequence",
                        v.type_name()
                    ));
                };
                let items = items.lock().clone();
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(self.from_json_checked(item, inner, Scalar::Any)?);
                }
                Ok(Value::vec(out))
            }
            _ => self.coerce_value(v, ty).map_err(|e| e.to_string()),
        }
    }

    /// A json object into a struct or a struct variant, the fields in declaration order like
    /// serde's derive.
    pub(super) fn struct_checked(
        self: &Arc<Self>,
        si: &StructInfo,
        map: &MapStore,
    ) -> std::result::Result<Value, String> {
        let mut values = Vec::with_capacity(si.json.len());
        for (slot, fname) in si.shape.fields.iter().enumerate() {
            let key = si
                .shape
                .renames
                .get(slot)
                .and_then(Option::as_ref)
                .unwrap_or(fname);
            let value = match map.get(&MapKey::Str((&**key).into())) {
                Some(raw) => self.from_json_checked(raw.clone(), &si.json[slot], Scalar::Any)?,
                None => match si.defaults.get(slot).and_then(Option::as_ref) {
                    Some(chunk) => self
                        .run_chunk(chunk, &[], &[], false)
                        .map_err(|e| format!("{e:#}"))?,
                    None if si.optional.get(slot).copied().unwrap_or(false) => Value::none(),
                    None => return Err(format!("missing field `{key}`")),
                },
            };
            values.push(value);
        }
        Ok(Value::structure(si.shape.clone(), values))
    }
}

fn short_name(def: &EnumDef) -> &str {
    def.name.rsplit("::").next().unwrap_or(&def.name)
}

fn expected_enum(def: &EnumDef) -> String {
    format!("invalid type, expected enum {}", short_name(def))
}

fn payload_kind(info: &EnumInfo, index: u16) -> &'static str {
    match info.variants.get(usize::from(index)) {
        Some(VariantPayload::Newtype(..)) => "newtype",
        Some(VariantPayload::Tuple(_)) => "tuple",
        Some(VariantPayload::Struct(_)) => "struct",
        _ => "unit",
    }
}

/// serde's `unknown variant` message lists the names like `one_of` does.
fn variant_named(def: &EnumDef, name: &str) -> std::result::Result<u16, String> {
    if let Some(i) = def.serde.names.iter().position(|n| &**n == name) {
        return u16::try_from(i).map_err(|e| e.to_string());
    }
    let names: Vec<String> = def.serde.names.iter().map(|n| format!("`{n}`")).collect();
    let expected = match names.as_slice() {
        [] => "there are no variants".to_string(),
        [one] => format!("expected {one}"),
        [a, b] => format!("expected {a} or {b}"),
        many => format!("expected one of {}", many.join(", ")),
    };
    Err(format!("unknown variant `{name}`, {expected}"))
}

// writing

/// A script enum value in its serde representation. `payload` is `None` for a unit variant,
/// otherwise the newtype value, the tuple array or the struct variant object.
pub(super) fn enum_to_json(
    def: &EnumDef,
    index: u16,
    payload: Option<JsonValue>,
) -> Result<JsonValue> {
    let name = def.serde_name(index).to_string();
    let object = |pairs: Vec<(String, JsonValue)>| {
        JsonValue::Object(pairs.into_iter().collect::<serde_json::Map<_, _>>())
    };
    Ok(match (&def.serde.repr, payload) {
        (SerdeRepr::External, None) => JsonValue::String(name),
        (SerdeRepr::External, Some(p)) => object(vec![(name, p)]),
        (SerdeRepr::Internal(tag) | SerdeRepr::Adjacent(tag, _), None) => {
            object(vec![(tag.to_string(), JsonValue::String(name))])
        }
        (SerdeRepr::Internal(tag), Some(JsonValue::Object(fields))) => {
            let mut pairs = vec![(tag.to_string(), JsonValue::String(name))];
            pairs.extend(fields);
            object(pairs)
        }
        (SerdeRepr::Internal(_), Some(_)) => bail!(
            "cannot serialize tagged newtype variant {}::{} containing a non-object",
            short_name(def),
            def.variant_name(index)
        ),
        (SerdeRepr::Adjacent(tag, content), Some(p)) => object(vec![
            (tag.to_string(), JsonValue::String(name)),
            (content.to_string(), p),
        ]),
        (SerdeRepr::Untagged, None) => JsonValue::Null,
        (SerdeRepr::Untagged, Some(p)) => p,
    })
}

/// Reads a script enum from the token stream. Each visit builds the plain json value, then the
/// enum reads from it inside the visit, so `serde_json` puts the line and column on an error.
pub(super) struct EnumVisitor<'a> {
    pub(super) info: &'a Arc<EnumInfo>,
    pub(super) cx: &'a ParseCx<'a>,
    /// an `Option<Enum>`, where null is `None`
    pub(super) optional: bool,
}

impl EnumVisitor<'_> {
    fn plain(&self) -> PlanVisitor<'_> {
        PlanVisitor {
            plan: &JsonPlan::Dynamic,
            cx: self.cx,
        }
    }

    fn read<E: serde::de::Error>(&self, raw: Value) -> std::result::Result<Value, E> {
        let Some(vm) = self.cx.vm else {
            return Ok(raw);
        };
        vm.enum_from_json(self.info, raw).map_err(E::custom)
    }
}

impl<'de> serde::de::Visitor<'de> for EnumVisitor<'_> {
    type Value = Value;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "enum {}", self.info.def.name)
    }

    fn visit_bool<E: serde::de::Error>(self, b: bool) -> std::result::Result<Value, E> {
        let raw = self.plain().visit_bool::<E>(b)?;
        self.read(raw)
    }

    fn visit_i64<E: serde::de::Error>(self, i: i64) -> std::result::Result<Value, E> {
        let raw = self.plain().visit_i64::<E>(i)?;
        self.read(raw)
    }

    fn visit_u64<E: serde::de::Error>(self, u: u64) -> std::result::Result<Value, E> {
        let raw = self.plain().visit_u64::<E>(u)?;
        self.read(raw)
    }

    fn visit_f64<E: serde::de::Error>(self, f: f64) -> std::result::Result<Value, E> {
        let raw = self.plain().visit_f64::<E>(f)?;
        self.read(raw)
    }

    fn visit_str<E: serde::de::Error>(self, s: &str) -> std::result::Result<Value, E> {
        self.read(Value::str(s))
    }

    fn visit_string<E: serde::de::Error>(self, s: String) -> std::result::Result<Value, E> {
        self.read(Value::str(s))
    }

    fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Value, E> {
        if self.optional {
            return Ok(Value::none());
        }
        self.read(Value::none())
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(
        self,
        seq: A,
    ) -> std::result::Result<Value, A::Error> {
        let raw = self.plain().visit_seq(seq)?;
        self.read(raw)
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(
        self,
        map: A,
    ) -> std::result::Result<Value, A::Error> {
        let raw = self.plain().visit_map(map)?;
        self.read(raw)
    }
}

/// A part of a `json!` value. An interpolated value goes through serde like `to_value`, an array
/// or object gathers parts already built.
pub(super) fn json_part(id: PathId, parts: Vec<Value>) -> Result<Value> {
    Ok(match id {
        PathId::JsonValue => {
            let value = parts.into_iter().next().unwrap_or(Value::Unit);
            super::json_bridge::json_to_pvalue(super::json_bridge::pvalue_to_json(&value)?)
        }
        PathId::JsonArray => Value::vec(parts),
        _ => {
            let mut map = MapStore::default();
            let mut pairs = parts.into_iter();
            while let (Some(key), Some(value)) = (pairs.next(), pairs.next()) {
                map.insert(MapKey::Str(key.display().into()), value);
            }
            Value::map_of(map)
        }
    })
}

/// A script enum value as json, its payload inside the enum's representation.
pub(super) fn user_enum_to_json(
    def: &EnumDef,
    variant: u16,
    payload: &[Value],
) -> Result<JsonValue> {
    use super::json_bridge::pvalue_to_json;
    let body = match payload {
        [] => None,
        [one] => Some(pvalue_to_json(one)?),
        many => Some(JsonValue::Array(
            many.iter().map(pvalue_to_json).collect::<Result<_>>()?,
        )),
    };
    enum_to_json(def, variant, body)
}
