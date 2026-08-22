//! Pattern lowering.

use std::sync::Arc;

use anyhow::Result;
use syn::{Expr, Lit, Pat};

use crate::interpreter::bytecode::{Op, PLit, PPat, PTag, PatInfo, Reg};
use crate::interpreter::enum_def::{EnumDef, builtin_enum, prelude_variant};
use crate::interpreter::numeric::IntWidth;

use super::{Compiler, Res, collect_pattern_names};

impl Compiler<'_> {
    pub(super) fn pattern_info(&mut self, pat: &Pat) -> Result<u16> {
        let mut names = Vec::new();
        collect_pattern_names(pat, &mut names);
        let mut binds = Vec::new();
        for n in names {
            let reg = self.alloc();
            self.define(&n, reg);
            binds.push((n, reg));
        }
        let lowered = self.lower_pattern(pat);
        let f = self.cur();
        f.pats.push(PatInfo {
            pat: lowered,
            binds,
        });
        Ok(u16::try_from(f.pats.len() - 1)?)
    }

    /// User enums first, builtin tables second. An unresolved path keeps its last segment and the
    /// runtime test falls back to the name.
    pub(super) fn variant_tag(&self, path: &syn::Path) -> PTag {
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        self.variant_tag_of(&segs)
    }

    pub(super) fn variant_tag_of(&self, segs: &[String]) -> PTag {
        PTag {
            name: segs.last().map(|s| Arc::from(s.as_str())),
            variant: self.resolve_variant(segs),
        }
    }

    pub(super) fn resolve_variant(&self, segs: &[String]) -> Option<(Arc<EnumDef>, u16)> {
        if let Ok(Res::TypeMember(canon, rest)) = self.resolve_path_res(segs)
            && let [variant] = rest.as_slice()
            && let Some(def) = self.ctx.resolver.enum_defs.get(&canon)
            && let Some(index) = def.variant_index(variant)
        {
            return Some((def.clone(), index));
        }
        match segs {
            [single] => prelude_variant(single).map(|(def, index)| (def.clone(), index)),
            [.., enum_name, variant] => {
                let def = builtin_enum(enum_name)?;
                Some((def.clone(), def.variant_index(variant)?))
            }
            [] => None,
        }
    }

    pub(super) fn lower_pattern(&self, pattern: &Pat) -> PPat {
        match pattern {
            Pat::Wild(_) => PPat::Wild,
            Pat::Rest(_) => PPat::Rest,
            Pat::Ident(ident) if is_unit_variant_ident(ident) => PPat::Path {
                tag: self.variant_tag_of(&[ident.ident.to_string()]),
            },
            Pat::Ident(ident) => PPat::Ident {
                name: ident.ident.to_string(),
                sub: ident
                    .subpat
                    .as_ref()
                    .map(|subpattern| Box::new(self.lower_pattern(&subpattern.1))),
            },
            Pat::Lit(literal) => lower_literal(&literal.lit),
            Pat::Paren(paren) => self.lower_pattern(&paren.pat),
            Pat::Reference(reference) => self.lower_pattern(&reference.pat),
            Pat::Type(typed) => self.lower_pattern(&typed.pat),
            Pat::Tuple(tuple) => {
                PPat::Tuple(tuple.elems.iter().map(|p| self.lower_pattern(p)).collect())
            }
            Pat::TupleStruct(tuple) => PPat::TupleStruct {
                tag: self.variant_tag(&tuple.path),
                elems: tuple.elems.iter().map(|p| self.lower_pattern(p)).collect(),
            },
            Pat::Path(path) => PPat::Path {
                tag: self.variant_tag(&path.path),
            },
            Pat::Struct(structure) => PPat::Struct {
                name: structure
                    .path
                    .segments
                    .last()
                    .map(|segment| segment.ident.to_string()),
                fields: structure
                    .fields
                    .iter()
                    .map(|field| {
                        let name = match &field.member {
                            syn::Member::Named(name) => name.to_string(),
                            syn::Member::Unnamed(index) => index.index.to_string(),
                        };
                        (name, self.lower_pattern(&field.pat))
                    })
                    .collect(),
            },
            Pat::Or(or) => PPat::Or(or.cases.iter().map(|p| self.lower_pattern(p)).collect()),
            Pat::Slice(slice) => {
                PPat::Slice(slice.elems.iter().map(|p| self.lower_pattern(p)).collect())
            }
            Pat::Range(range) => lower_range(range),
            _ => PPat::Unsupported,
        }
    }

    pub(super) fn bind_pattern_irrefutable(&mut self, pat: &Pat, reg: Reg) -> Result<()> {
        match pat {
            Pat::Ident(id) if id.subpat.is_none() => {
                self.define(&id.ident.to_string(), reg);
                Ok(())
            }
            Pat::Wild(_) => Ok(()),
            Pat::Type(t) => self.bind_pattern_irrefutable(&t.pat, reg),
            Pat::Paren(p) => self.bind_pattern_irrefutable(&p.pat, reg),
            Pat::Reference(r) => self.bind_pattern_irrefutable(&r.pat, reg),
            _ => {
                let matched = self.alloc();
                let pidx = self.pattern_info(pat)?;
                self.emit(Op::TestBind {
                    val: reg,
                    pat: pidx,
                    dst: matched,
                });
                Ok(())
            }
        }
    }

    // macros
}

// Real Rust tells a unit variant from a binding by name resolution, which we don't have. So an
// uppercase ident with no `ref`, `mut` or subpattern is a variant like `None`. Otherwise a bare
// `None` arm matches a `Some`.
pub(super) fn is_unit_variant_ident(id: &syn::PatIdent) -> bool {
    id.by_ref.is_none()
        && id.mutability.is_none()
        && id.subpat.is_none()
        && id
            .ident
            .to_string()
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase())
}

pub(super) fn lower_range(range: &syn::PatRange) -> PPat {
    // outer None is an unsupported literal, inner None is unbounded
    let endpoint = |e: &Option<Box<Expr>>| match e {
        Some(e) => endpoint_lit(e).map(Some),
        None => Some(None),
    };
    let (Some(lo), Some(hi)) = (endpoint(&range.start), endpoint(&range.end)) else {
        return PPat::Unsupported;
    };
    PPat::Range {
        lo,
        hi,
        inclusive: matches!(range.limits, syn::RangeLimits::Closed(_)),
    }
}

/// Including a negated number, seen through parens.
pub(super) fn endpoint_lit(e: &Expr) -> Option<PLit> {
    match e {
        Expr::Lit(l) => match &l.lit {
            Lit::Int(value) => value.base10_parse().ok().map(PLit::Int),
            Lit::Float(value) => value.base10_parse().ok().map(PLit::Float),
            Lit::Char(value) => Some(PLit::Char(value.value())),
            Lit::Byte(value) => Some(PLit::Int(i128::from(value.value()))),
            _ => None,
        },
        Expr::Unary(u) if matches!(u.op, syn::UnOp::Neg(_)) => match endpoint_lit(&u.expr) {
            Some(PLit::Int(n)) => Some(PLit::Int(-n)),
            Some(PLit::Float(f)) => Some(PLit::Float(-f)),
            _ => None,
        },
        Expr::Paren(p) => endpoint_lit(&p.expr),
        Expr::Group(g) => endpoint_lit(&g.expr),
        Expr::Path(p) if p.path.segments.len() == 2 => {
            let ty = p.path.segments[0].ident.to_string();
            let which = p.path.segments[1].ident.to_string();
            int_type_bound(&ty, &which).map(PLit::Int)
        }
        _ => None,
    }
}

/// Bounds outside i64 clamp to its range, which acts as unbounded.
pub(super) fn int_type_bound(ty: &str, which: &str) -> Option<i128> {
    let width = IntWidth::parse(ty)?;
    match which {
        "MIN" => Some(width.min()),
        "MAX" => Some(width.max()),
        _ => None,
    }
}

pub(super) fn lower_literal(literal: &Lit) -> PPat {
    match literal {
        Lit::Int(value) => value
            .base10_parse()
            .map_or(PPat::Unsupported, |value| PPat::Lit(PLit::Int(value))),
        Lit::Float(value) => value
            .base10_parse()
            .map_or(PPat::Unsupported, |value| PPat::Lit(PLit::Float(value))),
        Lit::Bool(value) => PPat::Lit(PLit::Bool(value.value)),
        Lit::Str(value) => PPat::Lit(PLit::Str(value.value())),
        Lit::Char(value) => PPat::Lit(PLit::Char(value.value())),
        Lit::Byte(value) => PPat::Lit(PLit::Int(i128::from(value.value()))),
        _ => PPat::Unsupported,
    }
}
