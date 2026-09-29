//! Where a format argument's type holds a `serde_json::Value`, so `{:?}` prints those parts as
//! json even inside a struct, map, vec or tuple.

use std::sync::Arc;

use super::{Infer, Ty};
use crate::interpreter::bytecode::JsonMask;
use crate::interpreter::compile::Ctx;

/// None when no part of the type is json.
pub(in crate::interpreter::compile) fn json_mask(ctx: &Ctx, ty: &Ty) -> Option<JsonMask> {
    Infer::new(ctx).mask(ty, &mut Vec::new())
}

impl Infer<'_, '_> {
    /// `seen` guards a recursive struct.
    fn mask(&self, ty: &Ty, seen: &mut Vec<Arc<str>>) -> Option<JsonMask> {
        let boxed = |m: JsonMask| Box::new(m);
        match ty {
            Ty::Json => Some(JsonMask::Here),
            Ty::Vec(item) => self.mask(item, seen).map(|m| JsonMask::Items(boxed(m))),
            Ty::Map(_, value, _) => self.mask(value, seen).map(|m| JsonMask::Values(boxed(m))),
            Ty::Option(inner) => self.mask(inner, seen).map(|m| JsonMask::Some(boxed(m))),
            Ty::Result(ok, err) => {
                let ok = self.mask(ok, seen).map(boxed);
                let err = self.mask(err, seen).map(boxed);
                (ok.is_some() || err.is_some()).then_some(JsonMask::Result(ok, err))
            }
            Ty::Tuple(items) => {
                let parts: Vec<_> = items
                    .iter()
                    .enumerate()
                    .filter_map(|(i, t)| Some((Arc::from(i.to_string()), self.mask(t, seen)?)))
                    .collect();
                (!parts.is_empty()).then_some(JsonMask::Parts(parts))
            }
            Ty::Struct(canon) => self.struct_mask(canon, seen),
            _ => None,
        }
    }

    fn struct_mask(&self, canon: &Arc<str>, seen: &mut Vec<Arc<str>>) -> Option<JsonMask> {
        // a user `Debug` or `Display` reads the fields itself, as the plain values its code expects.
        // Both are stored trait qualified, see `collect_impl_items`.
        let user_fmt = ["Display::fmt", "Debug::fmt"].iter().any(|m| {
            self.ctx
                .impl_sigs
                .contains_key(&(canon.to_string(), (*m).to_string()))
        });
        if user_fmt || seen.contains(canon) {
            return None;
        }
        let def = self.ctx.resolver.structs.get(&**canon)?;
        seen.push(canon.clone());
        // a tuple struct names its fields by position at runtime
        let parts: Vec<_> = def
            .ast
            .fields
            .iter()
            .enumerate()
            .filter_map(|(i, f)| {
                let name = f
                    .ident
                    .as_ref()
                    .map_or_else(|| i.to_string(), ToString::to_string);
                let ty = self.lower_in(&f.ty, def.module);
                Some((Arc::from(name), self.mask(&ty, seen)?))
            })
            .collect();
        seen.pop();
        (!parts.is_empty()).then_some(JsonMask::Parts(parts))
    }
}
