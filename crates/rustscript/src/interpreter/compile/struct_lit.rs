//! Struct literals and the field defaults they fill in.

use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;
use syn::Expr;

use crate::interpreter::bytecode::StructShape;
use crate::interpreter::bytecode::{Op, Reg, StructLit};
use crate::interpreter::serde_attrs::{serde_rename, serde_rename_all, serde_skip_none};

use super::{Compiler, idx16};

impl Compiler<'_> {
    pub(super) fn compile_struct_literal(&mut self, dst: Reg, s: &syn::ExprStruct) -> Result<()> {
        // a user struct resolves to its canonical name, anything else keeps the last segment
        let self_type = (s.path.segments.len() == 1 && s.path.segments[0].ident == "Self")
            .then_some(self.ctx.impl_type)
            .flatten();
        let resolved = self_type.map(Arc::<str>::from).or_else(|| {
            self.ctx
                .resolver
                .resolve_struct_key(self.ctx.module, &s.path)
        });
        let variant = resolved
            .is_none()
            .then(|| self.struct_variant(&s.path))
            .flatten();
        let (name, def) = if let Some(canon) = resolved {
            let def = self.ctx.resolver.structs.get(&canon).map(|d| d.ast.clone());
            (canon.to_string(), def)
        } else if let Some((_, _, fields)) = &variant {
            let bare = s
                .path
                .segments
                .last()
                .map(|seg| seg.ident.to_string())
                .unwrap_or_default();
            (bare, Some(fields.clone()))
        } else {
            let bare = s
                .path
                .segments
                .last()
                .map(|seg| seg.ident.to_string())
                .unwrap_or_default();
            (bare, None)
        };
        let mut written: Vec<(String, &Expr)> = Vec::new();
        for f in &s.fields {
            let key = match &f.member {
                syn::Member::Named(n) => n.to_string(),
                syn::Member::Unnamed(i) => i.index.to_string(),
            };
            written.push((key, &f.expr));
        }
        // With a `..rest` the shape lists every declared field, without one only the written
        // ones, the literal must have written all.
        let has_rest = s.rest.is_some();
        let (order, renames, skip_none) = literal_field_order(def.as_deref(), &written, has_rest);
        // reserve the window first so field temporaries don't break the packing
        let slots = order.len() + usize::from(has_rest);
        let base = self.cur().reg_top;
        for _ in 0..slots {
            self.alloc();
        }
        let held = self.cur().unwind_temps.len();
        for (i, fname) in order.iter().enumerate() {
            let dstf = base + idx16(i);
            match written.iter().find(|(k, _)| k == fname) {
                Some((_, e)) => {
                    self.compile_owned_into(dstf, e)?;
                    // a field already built drops when a later field panics, the struct op
                    // takes it out of the window once it runs
                    if self.ctx.has_drop && self.arg_owned(e) {
                        self.cur().hold_operand(dstf);
                    }
                }
                None => self.emit(Op::LoadUnit { dst: dstf }),
            }
        }
        if let Some(rest) = &s.rest {
            let reg = base + idx16(order.len());
            self.compile_owned_into(reg, rest)?;
            // the fields the literal wrote stay in a fresh base and drop with the statement
            if self.ctx.has_drop && self.temp_owned(rest) {
                self.cur().owned_temps.push(reg);
            }
        }
        self.cur().close_operands(held);
        let filled: Vec<bool> = order
            .iter()
            .map(|k| written.iter().any(|(w, _)| w == k))
            .collect();
        let info = {
            let fields: Vec<Arc<str>> = order.into_iter().map(Into::into).collect();
            let variant = variant.map(|(def, index, _)| (def, index));
            let shape = self.literal_shape(name, fields, renames, skip_none, variant);
            let f = self.cur();
            f.struct_lits.push(StructLit {
                shape,
                has_rest,
                filled: filled.into(),
            });
            idx16(f.struct_lits.len() - 1)
        };
        self.emit(Op::MakeStruct { dst, info, base });
        Ok(())
    }

    /// Shared with every literal of the same layout and the same enum variant.
    fn literal_shape(
        &mut self,
        name: String,
        fields: Vec<Arc<str>>,
        renames: Vec<Option<Arc<str>>>,
        skip_none: Vec<bool>,
        variant: Option<(Arc<crate::interpreter::enum_def::EnumDef>, u16)>,
    ) -> Arc<StructShape> {
        let variant_key = variant
            .as_ref()
            .map(|(def, index)| (Arc::as_ptr(def), *index));
        let known = self.shapes.iter().find(|s| {
            *s.name == name
                && s.fields == fields
                && s.renames == renames
                && s.skip_none == skip_none
                && s.variant.as_ref().map(|(d, i)| (Arc::as_ptr(d), *i)) == variant_key
        });
        if let Some(shared) = known {
            return shared.clone();
        }
        let type_id = self.ctx.resolver.type_id_of(&name);
        let mut built = StructShape::typed(name, type_id, fields, renames, skip_none);
        if let Some((def, index)) = variant {
            built = built.as_variant(def, index);
        }
        self.shapes.push(built.clone());
        built
    }

    /// `E::S { .. }` of a script enum, its definition, index, and its fields as a struct so
    /// the literal orders and renames them like a struct.
    fn struct_variant(
        &self,
        path: &syn::Path,
    ) -> Option<(
        Arc<crate::interpreter::enum_def::EnumDef>,
        u16,
        Rc<syn::ItemStruct>,
    )> {
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        let (def, index) = self.resolve_variant(&segs)?;
        if !def.user {
            return None;
        }
        let ast = self.ctx.resolver.enums.get(&def.name)?;
        let v = ast.variants.get(usize::from(index))?;
        let syn::Fields::Named(_) = &v.fields else {
            return None;
        };
        let ident = &v.ident;
        let fields = &v.fields;
        Some((
            def.clone(),
            index,
            Rc::new(syn::parse_quote!(struct #ident #fields)),
        ))
    }

    // patterns
}

/// `..rest` every declared field is listed.
pub(super) fn literal_field_order(
    def: Option<&syn::ItemStruct>,
    written: &[(String, &Expr)],
    has_rest: bool,
) -> (Vec<String>, Vec<Option<Arc<str>>>, Vec<bool>) {
    match def {
        Some(def) => {
            let mut ordered: Vec<String> = def
                .fields
                .iter()
                .filter_map(|f| f.ident.as_ref().map(std::string::ToString::to_string))
                .filter(|k| has_rest || written.iter().any(|(w, _)| w == k))
                .collect();
            for (k, _) in written {
                if !ordered.contains(k) {
                    ordered.push(k.clone());
                }
            }
            // so a serialized literal uses the same json keys as deserialize
            let rule = serde_rename_all(&def.attrs);
            let renames = ordered
                .iter()
                .map(|k| {
                    def.fields
                        .iter()
                        .find(|f| f.ident.as_ref().is_some_and(|i| i == k))
                        .and_then(serde_rename)
                        .or_else(|| rule.map(|r| r.apply(k)))
                        .map(Arc::<str>::from)
                })
                .collect();
            let skip_none = ordered
                .iter()
                .map(|k| {
                    def.fields
                        .iter()
                        .find(|f| f.ident.as_ref().is_some_and(|i| i == k))
                        .is_some_and(serde_skip_none)
                })
                .collect();
            (ordered, renames, skip_none)
        }
        None => (
            written.iter().map(|(k, _)| k.clone()).collect(),
            Vec::new(),
            Vec::new(),
        ),
    }
}
