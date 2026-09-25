//! Pattern lowering.

use std::sync::Arc;

use anyhow::Result;
use syn::{Expr, Lit, Pat};

use crate::interpreter::bytecode::{Op, PLit, PPat, PTag, PatInfo, Reg};
use crate::interpreter::enum_def::{EnumDef, builtin_enum, prelude_variant};
use crate::interpreter::numeric::IntWidth;
use crate::interpreter::resolver::bare;

use super::support::{pattern_borrows, pattern_owns};
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

    /// `pattern_info` over a known scrutinee. A bare `ref` binding over a variable aliases the
    /// variable and lowers as `_`, see `alias_ref_binding`.
    pub(super) fn pattern_info_over(&mut self, pat: &Pat, scrutinee: &Expr) -> Result<u16> {
        if self.alias_ref_binding(pat, scrutinee) {
            let wild = Pat::Wild(syn::PatWild {
                attrs: Vec::new(),
                underscore_token: syn::Token![_](proc_macro2::Span::call_site()),
            });
            return self.pattern_info(&wild);
        }
        self.pattern_info(pat)
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

    /// `scrutinee_mode` for one pattern.
    pub(super) fn pattern_mode(&mut self, pat: &Pat, expr: &Expr) -> (bool, bool) {
        self.scrutinee_mode(pattern_owns(pat), pattern_borrows(pat), expr)
    }

    /// The bindings that must not drop with their scope. A scrutinee that only lends keeps
    /// all of them, an owned one keeps the `ref` bindings.
    pub(super) fn exempt_binds(&mut self, pat: u16, takes: bool) {
        if takes {
            self.exempt_ref_binds(pat);
        } else {
            self.exempt_pattern_binds(pat);
        }
    }

    /// How a pattern reads its scrutinee, `(owned, by_ref)`. A `ref` binding wants the
    /// scrutinee's own storage. Next to a by value binding over a fresh value nobody else can
    /// see, the value moves like any owned scrutinee instead, the by value bindings take
    /// their parts and drop with the arm, and the `ref` ones lend theirs from the shell. Only
    /// a program with a `Drop` impl can tell, so only such a program takes this road.
    pub(super) fn scrutinee_mode(
        &mut self,
        owns: bool,
        borrows: bool,
        expr: &Expr,
    ) -> (bool, bool) {
        if owns && borrows && self.ctx.has_drop && self.fresh_scrutinee(expr) {
            return (true, false);
        }
        (owns && !borrows, borrows)
    }

    /// The `ref` bindings of a pattern over an owned scrutinee share what the shell still
    /// holds, so the shell alone drops it.
    fn exempt_ref_binds(&mut self, pat: u16) {
        let info = &self.cur().pats[usize::from(pat)];
        let mut names = Vec::new();
        ref_bind_names(&info.pat, &mut names);
        let regs: Vec<Reg> = info
            .binds
            .iter()
            .filter(|(name, _)| names.contains(name))
            .map(|(_, reg)| *reg)
            .collect();
        self.cur().drop_exempt.extend(regs);
    }

    pub(super) fn exempt_pattern_binds(&mut self, pat: u16) {
        let regs: Vec<Reg> = self.cur().pats[usize::from(pat)]
            .binds
            .iter()
            .map(|(_, reg)| *reg)
            .collect();
        self.cur().drop_exempt.extend(regs);
    }

    /// The bound parts of an owned scrutinee move to their bindings, so the shell drops
    /// without them. Only a program with a `Drop` impl can tell.
    pub(super) fn take_pattern_binds(&mut self, val: Reg, pat: u16) {
        if !self.ctx.has_drop {
            return;
        }
        self.emit(Op::TakeBinds { val, pat });
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
                by_ref: ident.by_ref.is_some(),
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

    /// A `_` parameter still owns its argument, which drops with the body like a named one.
    pub(super) fn hold_wild_param(&mut self, pat: &Pat, reg: Reg) {
        if is_wild(pat) {
            self.cur()
                .scope_order
                .last_mut()
                .expect("a scope is always open")
                .push(reg);
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

/// `_`, through a type ascription or parentheses.
pub(super) fn is_wild(pat: &Pat) -> bool {
    match pat {
        Pat::Type(t) => is_wild(&t.pat),
        Pat::Paren(p) => is_wild(&p.pat),
        Pat::Wild(_) => true,
        _ => false,
    }
}

fn ref_bind_names(pat: &PPat, out: &mut Vec<String>) {
    match pat {
        PPat::Ident { name, sub, by_ref } => {
            if *by_ref {
                out.push(name.clone());
            }
            if let Some(sub) = sub {
                ref_bind_names(sub, out);
            }
        }
        PPat::Tuple(items) | PPat::Or(items) | PPat::Slice(items) => {
            for item in items {
                ref_bind_names(item, out);
            }
        }
        PPat::TupleStruct { elems, .. } => {
            for item in elems {
                ref_bind_names(item, out);
            }
        }
        PPat::Struct { fields, .. } => {
            for (_, item) in fields {
                ref_bind_names(item, out);
            }
        }
        _ => {}
    }
}

/// Past this many alternatives a guarded arm tests its pattern whole, the guard runs once.
const MAX_GUARDED_ALTERNATIVES: usize = 64;

impl Compiler<'_> {
    /// A guard runs once per alternative of its arm's or-patterns, nested ones too, in the order
    /// rustc tries them, and each alternative binds on its own. So `(x, _) | (_, x) if x == 2`
    /// matches `(1, 2)` through the second one. The entries share the arm's binds and consts.
    pub(super) fn guarded_alternatives(&mut self, pat: u16) -> Vec<u16> {
        let info = &self.cur().pats[usize::from(pat)];
        let alts = or_alternatives(&info.pat);
        if alts.len() < 2 || alts.len() > MAX_GUARDED_ALTERNATIVES {
            return vec![pat];
        }
        let (binds, consts) = (info.binds.clone(), info.consts.clone());
        let f = self.cur();
        alts.into_iter()
            .map(|alt| {
                f.pats.push(PatInfo {
                    pat: alt,
                    binds: binds.clone(),
                    consts: consts.clone(),
                });
                idx16(f.pats.len() - 1)
            })
            .collect()
    }
}

/// The first element varies slowest, like rustc's expansion.
fn or_alternatives(pat: &PPat) -> Vec<PPat> {
    match pat {
        PPat::Or(alts) => alts.iter().flat_map(or_alternatives).collect(),
        PPat::Ident {
            name,
            sub: Some(sub),
            by_ref,
        } => or_alternatives(sub)
            .into_iter()
            .map(|sub| PPat::Ident {
                name: name.clone(),
                sub: Some(Box::new(sub)),
                by_ref: *by_ref,
            })
            .collect(),
        PPat::Tuple(elems) => alternatives_of_each(elems)
            .into_iter()
            .map(PPat::Tuple)
            .collect(),
        PPat::Slice(elems) => alternatives_of_each(elems)
            .into_iter()
            .map(PPat::Slice)
            .collect(),
        PPat::TupleStruct { tag, elems } => alternatives_of_each(elems)
            .into_iter()
            .map(|elems| PPat::TupleStruct {
                tag: tag.clone(),
                elems,
            })
            .collect(),
        PPat::Struct { name, fields } => {
            let pats: Vec<PPat> = fields.iter().map(|(_, p)| p.clone()).collect();
            alternatives_of_each(&pats)
                .into_iter()
                .map(|pats| PPat::Struct {
                    name: name.clone(),
                    fields: fields
                        .iter()
                        .map(|(key, _)| key.clone())
                        .zip(pats)
                        .collect(),
                })
                .collect()
        }
        other => vec![other.clone()],
    }
}

fn alternatives_of_each(elems: &[PPat]) -> Vec<Vec<PPat>> {
    let mut out = vec![Vec::new()];
    for elem in elems {
        let alts = or_alternatives(elem);
        out = out
            .into_iter()
            .flat_map(|prefix: Vec<PPat>| {
                alts.iter().map(move |alt| {
                    let mut next = prefix.clone();
                    next.push(alt.clone());
                    next
                })
            })
            .collect();
    }
    out
}
