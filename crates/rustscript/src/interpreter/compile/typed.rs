//! What the compiler reads off the inference table, the runtime carriers a method or a call
//! needs, `ScalarTy`, `DefaultIr` and `TypeIr`.

use std::sync::Arc;

use super::infer::Ty;
use super::{CollectTarget, Compiler};
use crate::interpreter::bytecode::{DefaultIr, ScalarTy};
use crate::interpreter::typeir::TypeIr;

impl Compiler<'_> {
    /// The path of the `From` impl on `canon` for a value of type `source`, keyed the way
    /// `collect_impl_items` registered it. The bare `from` is the runtime fallback for a source
    /// this pass could not type.
    pub(super) fn impl_path_for_from(&self, canon: &str, source: &Ty) -> Vec<String> {
        let known = from_keys(source)
            .into_iter()
            .map(|key| format!("from<{key}>"))
            .find(|name| {
                self.ctx
                    .impl_sigs
                    .contains_key(&(canon.to_string(), name.clone()))
            })
            .unwrap_or_else(|| "from".to_string());
        vec![canon.to_string(), known]
    }

    /// The collection a `collect` lands in, from its inferred result.
    pub(super) fn collect_target_of(&self, m: &syn::ExprMethodCall) -> Option<CollectTarget> {
        match self.types.of_node(m) {
            Ty::Str => Some(CollectTarget::Str),
            Ty::Map(..) => Some(CollectTarget::Map),
            Ty::Set(_) => Some(CollectTarget::Set),
            _ => None,
        }
    }

    /// The scalar a method carries at runtime. `parse` and the reductions carry their result,
    /// `concat` and a script method on a builtin carry the receiver.
    pub(super) fn method_scalar(&self, m: &syn::ExprMethodCall, method: &str) -> Option<ScalarTy> {
        let result = self.types.of_node(m);
        match method {
            "parse" => result.payload().to_scalar(),
            "unwrap_or_default" | "sum" | "product" | "collect" | "collect_string"
            | "collect_map" | "collect_set" => result.to_scalar(),
            "concat" => self.types.of(&m.receiver).item().to_scalar(),
            name if self.ctx.method_atoms.contains_key(name) => {
                self.types.of(&m.receiver).to_scalar()
            }
            _ => None,
        }
    }

    /// A `Default` built from an inferred type.
    pub(super) fn default_ir_of(&mut self, ty: &Ty) -> Option<DefaultIr> {
        Some(match ty {
            Ty::Int(w) => DefaultIr::Int(*w),
            Ty::F32 => DefaultIr::F32,
            Ty::F64 => DefaultIr::F64,
            Ty::Bool => DefaultIr::Bool,
            Ty::Char => DefaultIr::Char,
            Ty::Str => DefaultIr::Str,
            Ty::Unit => DefaultIr::Unit,
            Ty::Vec(_) => DefaultIr::Vec,
            Ty::Map(..) => DefaultIr::Map,
            Ty::Set(_) => DefaultIr::Set,
            Ty::Option(_) => DefaultIr::Opt,
            Ty::Tuple(items) => DefaultIr::Tuple(
                items
                    .iter()
                    .map(|item| self.default_ir_of(item))
                    .collect::<Option<Vec<_>>>()?,
            ),
            Ty::Struct(canon) => self.default_ir_struct(&canon.clone(), 0)?,
            Ty::Enum(canon) => self.default_ir_enum(&canon.clone())?,
            _ => return None,
        })
    }

    /// The coercion target of a typed json parse.
    pub(super) fn type_ir_of(ty: &Ty) -> TypeIr {
        match ty {
            Ty::Vec(t) => TypeIr::Vec(Arc::new(Self::type_ir_of(t))),
            Ty::Map(_, v) => TypeIr::MapValue(Arc::new(Self::type_ir_of(v))),
            Ty::Set(t) => TypeIr::Set(Arc::new(Self::type_ir_of(t))),
            Ty::Option(t) => TypeIr::Option(Arc::new(Self::type_ir_of(t))),
            Ty::Struct(canon) => TypeIr::Struct(canon.clone()),
            Ty::Generic(name) => TypeIr::Generic(name.clone()),
            _ => TypeIr::Dynamic,
        }
    }
}

/// The impl keys a source type can match, most specific first. `&str` and `String` sources
/// share one runtime value, so both are tried.
fn from_keys(ty: &Ty) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(full) = from_key(ty) {
        if let Some(base) = full.split(['<', '(']).next()
            && base != full
        {
            keys.push(base.to_string());
        }
        keys.insert(0, full);
    }
    if matches!(ty, Ty::Str) {
        keys.push("str".to_string());
    }
    keys
}

/// The text `from_type_key` writes for the same type.
fn from_key(ty: &Ty) -> Option<String> {
    Some(match ty {
        Ty::Int(w) => format!("{w:?}").to_lowercase(),
        Ty::F32 => "f32".to_string(),
        Ty::F64 => "f64".to_string(),
        Ty::Bool => "bool".to_string(),
        Ty::Char => "char".to_string(),
        Ty::Str => "String".to_string(),
        Ty::Unit => "()".to_string(),
        Ty::Vec(t) => format!("Vec<{}>", from_key(t)?),
        Ty::Set(t) => format!("HashSet<{}>", from_key(t)?),
        Ty::Map(k, v) => format!("HashMap<{},{}>", from_key(k)?, from_key(v)?),
        Ty::Option(t) => format!("Option<{}>", from_key(t)?),
        Ty::Result(t, e) => format!("Result<{},{}>", from_key(t)?, from_key(e)?),
        Ty::Tuple(items) => {
            let inner: Vec<String> = items.iter().map(from_key).collect::<Option<_>>()?;
            format!("({})", inner.join(","))
        }
        Ty::Struct(canon) | Ty::Enum(canon) => {
            crate::interpreter::resolver::bare(canon).to_string()
        }
        Ty::Named(name, args) => {
            let inner: Vec<String> = args.iter().map(from_key).collect::<Option<_>>()?;
            if inner.is_empty() {
                name.to_string()
            } else {
                format!("{name}<{}>", inner.join(","))
            }
        }
        _ => return None,
    })
}
