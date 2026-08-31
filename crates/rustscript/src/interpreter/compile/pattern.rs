//! Pattern lowering.

use std::sync::Arc;

use anyhow::Result;
use syn::{Expr, Lit, Pat};

use crate::interpreter::bytecode::{Op, PLit, PPat, PTag, PatInfo, Reg};
use crate::interpreter::enum_def::{EnumDef, builtin_enum, prelude_variant};
use crate::interpreter::numeric::IntWidth;
use crate::interpreter::resolver::bare;

use super::{Compiler, NameLoc, Res, collect_pattern_names, idx16};

/// Where the value of a constant used as a pattern comes from.
enum ConstSource {
    Local(Reg),
    Cell(Reg),
    Upvalue(u16),
    Global(u32),
}

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
        // The loads land right before the `TestBind` every caller emits next.
        let mut consts = Vec::new();
        let lowered = self.lower_pattern(pat, &mut consts);
        let f = self.cur();
        f.pats.push(PatInfo {
            pat: lowered,
            binds,
            consts,
        });
        Ok(u16::try_from(f.pats.len() - 1)?)
    }

    /// The bindings of a pattern over a borrowed scrutinee hold borrowed handles, so scope end
    /// must not drop them.
    /// Bindings out of a scrutinee that holds a `RefCell` guard keep the borrow alive until
    /// their scope ends, `Ok(g)` of a `try_borrow`.
    pub(super) fn guard_pattern_binds(&mut self, pat: u16) {
        let regs: Vec<Reg> = self.cur().pats[usize::from(pat)]
            .binds
            .iter()
            .map(|(_, reg)| *reg)
            .collect();
        let f = self.cur();
        f.guard_regs.extend(regs);
        f.has_guards = true;
    }

    pub(super) fn exempt_pattern_binds(&mut self, pat: u16) {
        let regs: Vec<Reg> = self.cur().pats[usize::from(pat)]
            .binds
            .iter()
            .map(|(_, reg)| *reg)
            .collect();
        self.cur().drop_exempt.extend(regs);
    }

    /// User enums first, builtin tables second. An unresolved path keeps its last segment and the
    /// runtime test falls back to the name.
    pub(super) fn variant_tag(&self, path: &syn::Path) -> PTag {
        self.variant_tag_of(&path_segments(path))
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

    pub(super) fn lower_pattern(&mut self, pattern: &Pat, consts: &mut Vec<Reg>) -> PPat {
        match pattern {
            Pat::Wild(_) => PPat::Wild,
            Pat::Rest(_) => PPat::Rest,
            Pat::Ident(ident) if is_unit_variant_ident(ident) => {
                self.path_pattern(&[ident.ident.to_string()], consts)
            }
            Pat::Ident(ident) => PPat::Ident {
                name: ident.ident.to_string(),
                sub: ident
                    .subpat
                    .as_ref()
                    .map(|subpattern| Box::new(self.lower_pattern(&subpattern.1, consts))),
            },
            Pat::Lit(literal) => lower_literal(&literal.lit),
            Pat::Paren(paren) => self.lower_pattern(&paren.pat, consts),
            Pat::Reference(reference) => self.lower_pattern(&reference.pat, consts),
            Pat::Type(typed) => self.lower_pattern(&typed.pat, consts),
            Pat::Tuple(tuple) => PPat::Tuple(self.lower_patterns(&tuple.elems, consts)),
            Pat::TupleStruct(tuple) => PPat::TupleStruct {
                tag: self.variant_tag(&tuple.path),
                elems: self.lower_patterns(&tuple.elems, consts),
            },
            Pat::Path(path) => {
                let segs = path_segments(&path.path);
                self.path_pattern(&segs, consts)
            }
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
                        (name, self.lower_pattern(&field.pat, consts))
                    })
                    .collect(),
            },
            Pat::Or(or) => PPat::Or(self.lower_patterns(&or.cases, consts)),
            Pat::Slice(slice) => PPat::Slice(self.lower_patterns(&slice.elems, consts)),
            Pat::Range(range) => lower_range(range),
            _ => PPat::Unsupported,
        }
    }

    fn lower_patterns<'p>(
        &mut self,
        pats: impl IntoIterator<Item = &'p Pat>,
        consts: &mut Vec<Reg>,
    ) -> Vec<PPat> {
        pats.into_iter()
            .map(|p| self.lower_pattern(p, consts))
            .collect()
    }

    /// A variant wins over a constant of the same name, then a constant compiles to an equality
    /// test like real Rust. An unresolved path stays a tag the runtime tests by name.
    fn path_pattern(&mut self, segs: &[String], consts: &mut Vec<Reg>) -> PPat {
        if self.resolve_variant(segs).is_none()
            && let Some(pat) = self.const_pattern(segs, consts)
        {
            return pat;
        }
        PPat::Path {
            tag: self.variant_tag_of(segs),
        }
    }

    /// `None` when the path is no constant this compiler knows.
    fn const_pattern(&mut self, segs: &[String], consts: &mut Vec<Reg>) -> Option<PPat> {
        if let [ty, which] = segs
            && let Some(bound) = int_type_bound(ty, which)
        {
            return Some(PPat::Lit(PLit::Int(bound)));
        }
        let source = self.const_source(segs)?;
        let dst = self.alloc();
        match source {
            ConstSource::Local(src) => self.emit(Op::Move { dst, src }),
            ConstSource::Cell(cell) => self.emit(Op::LoadCell { dst, cell }),
            ConstSource::Upvalue(idx) => self.emit(Op::LoadUpvalue { dst, idx }),
            ConstSource::Global(idx) => self.emit(Op::LoadGlobal { dst, idx }),
        }
        consts.push(dst);
        Some(PPat::Const(idx16(consts.len() - 1)))
    }

    /// Block level `const` items are plain locals, module level ones are globals.
    fn const_source(&mut self, segs: &[String]) -> Option<ConstSource> {
        if let [name] = segs
            && self.block_const(name)
        {
            return match self.resolve(name) {
                NameLoc::Local(reg) => Some(ConstSource::Local(reg)),
                NameLoc::Cell(cell) => Some(ConstSource::Cell(cell)),
                NameLoc::Upvalue(idx) => Some(ConstSource::Upvalue(idx)),
                NameLoc::None => None,
            };
        }
        self.const_global(segs).map(ConstSource::Global)
    }

    /// The global slot of a module `const`, a `static`, or an impl `Type::NAME`.
    fn const_global(&self, segs: &[String]) -> Option<u32> {
        match self.resolve_path_res(segs).ok()? {
            Res::Const(idx) => Some(idx),
            Res::TypeMember(canon, rest) => {
                let [name] = rest.as_slice() else {
                    return None;
                };
                let key = format!("{}::{name}", bare(&canon));
                self.ctx
                    .resolver
                    .modules
                    .iter()
                    .find_map(|syms| syms.consts.get(&key).copied())
            }
            _ => None,
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
pub(super) fn path_segments(path: &syn::Path) -> Vec<String> {
    path.segments.iter().map(|s| s.ident.to_string()).collect()
}

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
