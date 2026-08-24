//! The expression walks. Each arm gives its children an expected type and returns its own.

use std::sync::Arc;

use syn::{Expr, Lit, Pat};

use super::super::support::{bin_kind, is_assign_op};
use super::{Infer, Ty, lit_ty};
use crate::interpreter::bytecode::BinKind;
use crate::interpreter::numeric::IntWidth;
use crate::interpreter::resolver::Res;

/// The name a one binding tuple struct pattern binds, `Ok(config)` gives `config`.
fn single_binding(pat: &Pat, wrapper: &str) -> Option<String> {
    let Pat::TupleStruct(ts) = pat else {
        return None;
    };
    if ts.path.segments.last()?.ident != wrapper || ts.elems.len() != 1 {
        return None;
    }
    match ts.elems.first()? {
        Pat::Ident(id) => Some(id.ident.to_string()),
        _ => None,
    }
}

/// True when the body is exactly `name`, looking through parens and a block tail.
fn returns_binding(body: &Expr, name: &str) -> bool {
    match body {
        Expr::Path(p) => p.qself.is_none() && p.path.is_ident(name),
        Expr::Paren(p) => returns_binding(&p.expr, name),
        Expr::Group(g) => returns_binding(&g.expr, name),
        Expr::Block(b) => match b.block.stmts.last() {
            Some(syn::Stmt::Expr(tail, None)) => returns_binding(tail, name),
            _ => false,
        },
        _ => false,
    }
}

/// What the scrutinee of a `match` should be, given what the whole `match` must produce.
///
/// A `match parse() { Ok(v) => v, Err(e) => .. }` hands its payload straight out, so the
/// expectation on the arm is the payload of the scrutinee, the same way `?` carries an annotation
/// into the parse under it. Without this the scrutinee is inferred blind and a `serde_json::from_str`
/// there has no target type to parse into, which is silent, the fields come back empty.
fn match_scrutinee_expectation(m: &syn::ExprMatch, expected: &Ty) -> Ty {
    if matches!(expected, Ty::Unknown) {
        return Ty::Unknown;
    }
    for arm in &m.arms {
        if let Some(name) = single_binding(&arm.pat, "Ok")
            && returns_binding(&arm.body, &name)
        {
            return Ty::result(expected.clone(), Ty::Unknown);
        }
        if let Some(name) = single_binding(&arm.pat, "Some")
            && returns_binding(&arm.body, &name)
        {
            return Ty::option(expected.clone());
        }
    }
    Ty::Unknown
}

impl Infer<'_, '_> {
    /// The type of `e`, recorded. `expected` flows down into literals and constructors.
    pub(super) fn expr(&mut self, e: &Expr, expected: &Ty) -> Ty {
        let ty = self.expr_inner(e, expected);
        let ty = self.vars.meet(&ty, expected);
        self.types.insert(std::ptr::from_ref(e), ty.clone());
        let node: *const () = match e {
            Expr::MethodCall(m) => std::ptr::from_ref(m).cast(),
            Expr::Call(c) => std::ptr::from_ref(c).cast(),
            Expr::Macro(m) => std::ptr::from_ref(&m.mac).cast(),
            Expr::Closure(c) => std::ptr::from_ref(c).cast(),
            _ => return ty,
        };
        self.nodes.insert(node, ty.clone());
        ty
    }

    fn expr_inner(&mut self, e: &Expr, expected: &Ty) -> Ty {
        match e {
            Expr::Paren(p) => self.expr(&p.expr, expected),
            Expr::Group(g) => self.expr(&g.expr, expected),
            Expr::Reference(r) => self.expr(&r.expr, expected),
            Expr::Lit(l) => self.literal(e, expected).unwrap_or_else(|| lit_ty(&l.lit)),
            Expr::Path(p) => self.path_value(p, expected),
            Expr::Unary(u) => match u.op {
                syn::UnOp::Deref(_) | syn::UnOp::Not(_) | syn::UnOp::Neg(_) => {
                    self.expr(&u.expr, expected)
                }
                _ => Ty::Unknown,
            },
            Expr::Binary(b) => self.binary(b, expected),
            Expr::Assign(a) => {
                let target = self.expr(&a.left, &Ty::Unknown);
                let value = self.expr(&a.right, &target);
                self.vars.unify(&target, &value);
                Ty::Unit
            }
            Expr::Cast(c) => {
                self.expr(&c.expr, &Ty::Unknown);
                self.lower(&c.ty)
            }
            Expr::Block(b) => self.block(&b.block, expected),
            Expr::Unsafe(u) => self.block(&u.block, expected),
            Expr::Async(a) => {
                let inner = self.block(&a.block, &Ty::Unknown);
                Ty::Named(Arc::from("JoinHandle"), vec![inner])
            }
            Expr::If(_)
            | Expr::Match(_)
            | Expr::While(_)
            | Expr::Loop(_)
            | Expr::ForLoop(_)
            | Expr::Break(_)
            | Expr::Return(_) => self.flow_expr(e, expected),
            Expr::Try(t) => {
                // the payload expectation reaches a `from_str` or a generic call under the `?`
                let inner = self.expr(&t.expr, &Ty::result(expected.clone(), Ty::Unknown));
                inner.payload()
            }
            Expr::Await(a) => {
                let inner = self.expr(&a.base, &Ty::Unknown);
                match inner {
                    Ty::Named(name, args) if &*name == "JoinHandle" => Ty::result(
                        args.into_iter().next().unwrap_or(Ty::Unknown),
                        Ty::named("JoinError"),
                    ),
                    other => other,
                }
            }
            Expr::Let(l) => {
                let scrutinee = self.expr(&l.expr, &Ty::Unknown);
                self.bind_pat(&l.pat, &scrutinee);
                Ty::Bool
            }
            Expr::Closure(c) => self.closure(c, expected),
            Expr::Call(c) => self.call(c, expected),
            Expr::MethodCall(m) => self.method_call(m, expected),
            Expr::Macro(m) => self.macro_ty(&m.mac, expected),
            Expr::Struct(s) => self.struct_literal(s),
            Expr::Tuple(_)
            | Expr::Array(_)
            | Expr::Repeat(_)
            | Expr::Index(_)
            | Expr::Field(_)
            | Expr::Range(_) => self.data_expr(e, expected),
            _ => Ty::Unknown,
        }
    }

    /// Tuples, arrays, ranges and the reads out of them.
    fn data_expr(&mut self, e: &Expr, expected: &Ty) -> Ty {
        match e {
            Expr::Tuple(t) => {
                let wants: Vec<Ty> = match expected {
                    Ty::Tuple(items) if items.len() == t.elems.len() => items.clone(),
                    _ => vec![Ty::Unknown; t.elems.len()],
                };
                Ty::Tuple(
                    t.elems
                        .iter()
                        .zip(wants)
                        .map(|(e, want)| self.expr(e, &want))
                        .collect(),
                )
            }
            Expr::Array(a) => {
                let mut item = expected.item();
                for e in &a.elems {
                    let got = self.expr(e, &item);
                    item = self.vars.meet(&item, &got);
                }
                Ty::vec(item)
            }
            Expr::Repeat(r) => {
                let item = self.expr(&r.expr, &expected.item());
                self.expr(&r.len, &Ty::usize());
                Ty::vec(item)
            }
            Expr::Index(ix) => self.index(ix),
            Expr::Field(f) => {
                let base = self.expr(&f.base, &Ty::Unknown);
                match (&base, &f.member) {
                    (Ty::Struct(canon), member) => self.field_ty(canon, member),
                    (Ty::Tuple(items), syn::Member::Unnamed(i)) => {
                        items.get(i.index as usize).cloned().unwrap_or(Ty::Unknown)
                    }
                    _ => Ty::Unknown,
                }
            }
            Expr::Range(r) => {
                let mut item = match expected {
                    Ty::Range(t) | Ty::Iter(t) => (**t).clone(),
                    _ => Ty::Unknown,
                };
                if let Some(start) = &r.start {
                    let got = self.expr(start, &item);
                    item = self.vars.meet(&item, &got);
                }
                if let Some(end) = &r.end {
                    let got = self.expr(end, &item);
                    item = self.vars.meet(&item, &got);
                }
                Ty::Range(Box::new(item))
            }
            _ => Ty::Unknown,
        }
    }

    /// The control flow expressions.
    fn flow_expr(&mut self, e: &Expr, expected: &Ty) -> Ty {
        match e {
            Expr::If(i) => self.if_expr(i, expected),
            Expr::Match(m) => {
                let want = match_scrutinee_expectation(m, expected);
                let scrutinee = self.expr(&m.expr, &want);
                let mut out = Ty::Unknown;
                for arm in &m.arms {
                    self.push();
                    self.bind_pat(&arm.pat, &scrutinee);
                    let body = self.expr(&arm.body, expected);
                    self.pop();
                    out = self.vars.meet(&out, &body);
                }
                out
            }
            Expr::While(w) => {
                self.push();
                self.cond(&w.cond);
                self.block(&w.body, &Ty::Unknown);
                self.pop();
                Ty::Unit
            }
            Expr::Loop(l) => {
                self.loops.push(expected.clone());
                self.block(&l.body, &Ty::Unknown);
                self.loops.pop().unwrap_or(Ty::Unit)
            }
            Expr::ForLoop(f) => {
                let iterable = self.expr(&f.expr, &Ty::Unknown);
                self.push();
                let item = iterable.item();
                self.bind_pat(&f.pat, &item);
                self.block(&f.body, &Ty::Unknown);
                self.pop();
                Ty::Unit
            }
            Expr::Break(b) => {
                if let Some(value) = &b.expr {
                    let want = self.loops.last().cloned().unwrap_or(Ty::Unknown);
                    let got = self.expr(value, &want);
                    if let Some(slot) = self.loops.last_mut() {
                        *slot = got;
                    }
                }
                Ty::Unknown
            }
            Expr::Return(r) => {
                if let Some(value) = &r.expr {
                    let ret = self.ret.clone();
                    self.expr(value, &ret);
                }
                Ty::Unknown
            }
            _ => Ty::Unknown,
        }
    }

    /// A literal under an expectation. `None` for a non numeric literal.
    pub(super) fn literal(&mut self, e: &Expr, expected: &Ty) -> Option<Ty> {
        let Expr::Lit(l) = e else { return None };
        match &l.lit {
            Lit::Int(int) => Some(match IntWidth::parse(int.suffix()) {
                Some(w) => Ty::Int(w),
                None => match expected {
                    Ty::Int(_) | Ty::IntVar(_) => expected.clone(),
                    _ => self.vars.fresh_int(),
                },
            }),
            Lit::Float(float) => Some(match float.suffix() {
                "f32" => Ty::F32,
                "f64" => Ty::F64,
                _ => match expected {
                    Ty::F32 | Ty::F64 | Ty::FloatVar(_) => expected.clone(),
                    _ => self.vars.fresh_float(),
                },
            }),
            _ => None,
        }
    }

    fn cond(&mut self, cond: &Expr) {
        // let chains bind for the rest of the condition and the body
        if let Expr::Binary(b) = cond
            && matches!(b.op, syn::BinOp::And(_))
        {
            self.cond(&b.left);
            self.cond(&b.right);
            self.types.insert(std::ptr::from_ref(cond), Ty::Bool);
            return;
        }
        self.expr(cond, &Ty::Bool);
    }

    fn if_expr(&mut self, i: &syn::ExprIf, expected: &Ty) -> Ty {
        self.push();
        self.cond(&i.cond);
        let then = self.block_inner(&i.then_branch, expected);
        self.pop();
        match &i.else_branch {
            Some((_, other)) => {
                let alt = self.expr(other, &then);
                self.vars.meet(&then, &alt)
            }
            None => Ty::Unit,
        }
    }

    fn binary(&mut self, b: &syn::ExprBinary, expected: &Ty) -> Ty {
        if is_assign_op(&b.op) {
            let target = self.expr(&b.left, &Ty::Unknown);
            let want = match bin_kind(&b.op) {
                Some(BinKind::Shl | BinKind::Shr) => Ty::Unknown,
                _ => target.clone(),
            };
            let value = self.expr(&b.right, &want);
            if !matches!(bin_kind(&b.op), Some(BinKind::Shl | BinKind::Shr)) {
                self.vars.unify(&target, &value);
            }
            return Ty::Unit;
        }
        match bin_kind(&b.op) {
            Some(
                BinKind::Eq | BinKind::Ne | BinKind::Lt | BinKind::Le | BinKind::Gt | BinKind::Ge,
            ) => {
                let left = self.expr(&b.left, &Ty::Unknown);
                let right = self.expr(&b.right, &left);
                self.vars.unify(&left, &right);
                Ty::Bool
            }
            Some(BinKind::Shl | BinKind::Shr) => {
                let left = self.expr(&b.left, expected);
                self.expr(&b.right, &Ty::Unknown);
                left
            }
            Some(_) => {
                let left = self.expr(&b.left, expected);
                let want = if left.is_numeric() || matches!(left, Ty::Str) {
                    left.clone()
                } else {
                    expected.clone()
                };
                let right = self.expr(&b.right, &want);
                if left.is_numeric() && right.is_numeric() {
                    self.vars.unify(&left, &right);
                }
                match (&left, &right) {
                    (Ty::Str, _) => Ty::Str,
                    (Ty::Unknown, other) => other.clone(),
                    (known, _) => known.clone(),
                }
            }
            None => {
                // `&&` and `||`
                self.expr(&b.left, &Ty::Bool);
                self.expr(&b.right, &Ty::Bool);
                Ty::Bool
            }
        }
    }

    fn index(&mut self, ix: &syn::ExprIndex) -> Ty {
        let base = self.expr(&ix.expr, &Ty::Unknown);
        let by_range = matches!(&*ix.index, Expr::Range(_));
        match &base {
            Ty::Vec(item) => {
                let key = if by_range {
                    Ty::Range(Box::new(Ty::usize()))
                } else {
                    Ty::usize()
                };
                self.expr(&ix.index, &key);
                if by_range {
                    base.clone()
                } else {
                    (**item).clone()
                }
            }
            Ty::Map(key, value) => {
                self.expr(&ix.index, key);
                (**value).clone()
            }
            Ty::Str => {
                self.expr(&ix.index, &Ty::Range(Box::new(Ty::usize())));
                Ty::Str
            }
            Ty::Json => {
                self.expr(&ix.index, &Ty::Unknown);
                Ty::Json
            }
            // everything else indexes by position, regex captures and slices included
            _ => {
                let key = if by_range {
                    Ty::Range(Box::new(Ty::usize()))
                } else {
                    Ty::usize()
                };
                self.expr(&ix.index, &key);
                Ty::Unknown
            }
        }
    }

    fn closure(&mut self, c: &syn::ExprClosure, expected: &Ty) -> Ty {
        let (want_params, want_ret) = match expected {
            Ty::Closure(params, ret) => (params.clone(), (**ret).clone()),
            _ => (Vec::new(), Ty::Unknown),
        };
        self.push();
        let mut params = Vec::new();
        for (i, input) in c.inputs.iter().enumerate() {
            let want = want_params.get(i).cloned().unwrap_or(Ty::Unknown);
            let ty = match input {
                Pat::Type(t) => {
                    let declared = self.lower(&t.ty);
                    self.vars.meet(&declared, &want)
                }
                _ => want,
            };
            self.bind_pat(input, &ty);
            params.push(ty);
        }
        let ret = match &c.output {
            syn::ReturnType::Type(_, ty) => self.lower(ty),
            syn::ReturnType::Default => want_ret,
        };
        let saved_ret = std::mem::replace(&mut self.ret, ret.clone());
        let saved_loops = std::mem::take(&mut self.loops);
        let body = self.expr(&c.body, &ret);
        self.loops = saved_loops;
        self.ret = saved_ret;
        self.pop();
        Ty::Closure(params, Box::new(self.vars.meet(&ret, &body)))
    }

    fn struct_literal(&mut self, s: &syn::ExprStruct) -> Ty {
        let canon = self.struct_key(&s.path);
        for field in &s.fields {
            let want = canon
                .as_ref()
                .map_or(Ty::Unknown, |canon| self.field_ty(canon, &field.member));
            self.expr(&field.expr, &want);
        }
        let ty = canon.clone().map_or(Ty::Unknown, Ty::Struct);
        if let Some(rest) = &s.rest {
            self.expr(rest, &ty);
        }
        if canon.is_some() {
            return ty;
        }
        // an enum struct variant, `Shape::Rect { w, h }`
        let segs: Vec<String> = s
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        match self.ctx.resolver.resolve(self.ctx.module, &segs) {
            Ok(Res::TypeMember(canon, _)) if self.ctx.resolver.enums.contains_key(&canon) => {
                Ty::Enum(canon)
            }
            _ => Ty::Unknown,
        }
    }
}
