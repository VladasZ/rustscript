//! Struct literals and the field defaults they fill in.

use std::sync::Arc;

use anyhow::Result;
use syn::Expr;

use crate::interpreter::bytecode::StructShape;
use crate::interpreter::bytecode::{Op, Reg, StructLit};
use crate::interpreter::serde_attrs::serde_rename;

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
        let (name, def) = if let Some(canon) = resolved {
            let def = self.ctx.resolver.structs.get(&canon).map(|d| d.ast.clone());
            (canon.to_string(), def)
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
        let (order, renames) = literal_field_order(def.as_deref(), &written, has_rest);
        // reserve the window first so field temporaries don't break the packing
        let slots = order.len() + usize::from(has_rest);
        let base = self.cur().reg_top;
        for _ in 0..slots {
            self.alloc();
        }
        for (i, fname) in order.iter().enumerate() {
            let dstf = base + idx16(i);
            match written.iter().find(|(k, _)| k == fname) {
                Some((_, e)) => {
                    self.compile_owned_into(dstf, e)?;
                }
                None => self.emit(Op::LoadUnit { dst: dstf }),
            }
        }
        if let Some(rest) = &s.rest {
            self.compile_owned_into(base + idx16(order.len()), rest)?;
        }
        let filled: Vec<bool> = order
            .iter()
            .map(|k| written.iter().any(|(w, _)| w == k))
            .collect();
        let info = {
            let fields: Vec<Arc<str>> = order.into_iter().map(Into::into).collect();
            let known = self
                .shapes
                .iter()
                .find(|s| *s.name == name && s.fields == fields && s.renames == renames);
            let shape = if let Some(shared) = known {
                shared.clone()
            } else {
                let type_id = self.ctx.resolver.type_id_of(&name);
                let built = StructShape::typed(name, type_id, fields, renames);
                self.shapes.push(built.clone());
                built
            };
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

    // patterns
}

/// `..rest` every declared field is listed.
pub(super) fn literal_field_order(
    def: Option<&syn::ItemStruct>,
    written: &[(String, &Expr)],
    has_rest: bool,
) -> (Vec<String>, Vec<Option<Arc<str>>>) {
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
            let renames = ordered
                .iter()
                .map(|k| {
                    def.fields
                        .iter()
                        .find(|f| f.ident.as_ref().is_some_and(|i| i == k))
                        .and_then(serde_rename)
                        .map(Arc::<str>::from)
                })
                .collect();
            (ordered, renames)
        }
        None => (written.iter().map(|(k, _)| k.clone()).collect(), Vec::new()),
    }
}
