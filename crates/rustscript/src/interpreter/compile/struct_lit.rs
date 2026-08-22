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
                    self.field_default_hint(e, def.as_deref(), fname);
                    self.compile_into(dstf, e)?;
                }
                None => self.emit(Op::LoadUnit { dst: dstf }),
            }
        }
        if let Some(rest) = &s.rest {
            self.rest_default_hint(rest, self_type, &s.path);
            self.compile_into(base + idx16(order.len()), rest)?;
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

impl Compiler<'_> {
    /// `field: Default::default()` takes the type from the struct definition
    pub(super) fn field_default_hint(
        &mut self,
        e: &Expr,
        def: Option<&syn::ItemStruct>,
        fname: &str,
    ) {
        if let Some(call) = bare_default_call(e)
            && let Some(field_ty) = def.and_then(|def| {
                def.fields
                    .iter()
                    .find(|f| f.ident.as_ref().is_some_and(|i| i == fname))
                    .map(|f| f.ty.clone())
            })
            && let Some(ir) = self.default_ir(&field_ty)
        {
            self.default_calls.insert(std::ptr::from_ref(call), ir);
        }
    }

    pub(super) fn rest_default_hint(
        &mut self,
        rest: &Expr,
        self_type: Option<&str>,
        path: &syn::Path,
    ) {
        if let Some(call) = bare_default_call(rest)
            && let Some(canon) = self_type
                .map(Arc::<str>::from)
                .or_else(|| self.ctx.resolver.resolve_struct_key(self.ctx.module, path))
            && let Some(ir) = self.default_ir_for_struct(&canon)
        {
            self.default_calls.insert(std::ptr::from_ref(call), ir);
        }
    }
}

/// Declaration order when the struct is known, with the serde rename of each field. With a
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

/// Parentheses stripped.
pub(super) fn bare_default_call(e: &Expr) -> Option<&syn::ExprCall> {
    match e {
        Expr::Paren(p) => bare_default_call(&p.expr),
        Expr::Group(g) => bare_default_call(&g.expr),
        Expr::Call(c) if c.args.is_empty() => {
            let Expr::Path(p) = &*c.func else { return None };
            let segs: Vec<String> = p
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            (p.qself.is_none() && segs == ["Default", "default"]).then_some(c)
        }
        _ => None,
    }
}
