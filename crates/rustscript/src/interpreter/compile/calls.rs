//! Calls, closures, assignment, struct literals, and patterns. Split from the compiler.

use std::rc::Rc;

use anyhow::{Result, bail};
use syn::{Expr, Pat, UnOp};

use crate::interpreter::bytecode::{
    BinKind, CapSource, Member, Op, PatInfo, Reg,
    StructLit,
};

use super::*;

impl Compiler<'_> {
    /// Compile arguments into a fresh contiguous register window and return its
    /// base. The window is reserved first so an argument's own temporaries,
    /// allocated above it, cannot break the packing.
    pub(super) fn compile_args<'e>(&mut self, args: impl Iterator<Item = &'e Expr>) -> Result<Reg> {
        let list: Vec<&Expr> = args.collect();
        let base = self.cur().reg_top;
        for _ in 0..list.len() {
            self.alloc();
        }
        for (i, a) in list.iter().enumerate() {
            self.compile_into(base + i as Reg, a)?;
        }
        Ok(base)
    }

    pub(super) fn compile_call(&mut self, dst: Reg, c: &syn::ExprCall) -> Result<()> {
        let path = match &*c.func {
            Expr::Path(p) => &p.path,
            _ => bail!("cannot call this kind of expression"),
        };
        let coerce = path.segments.last().and_then(first_generic_type).map(|t| Rc::new(t.clone()));
        let argc = c.args.len() as u16;

        if path.segments.len() == 1 {
            let name = path.segments[0].ident.to_string();
            // A local closure value called directly.
            if let NameLoc::Local(reg) = self.resolve(&name) {
                let base = self.compile_args(c.args.iter())?;
                self.emit(Op::CallValue { dst, callee: reg, base, argc });
                return Ok(());
            }
            // A known top level function, called directly.
            if let Some(&idx) = self.ctx.fn_index.get(&name) {
                let base = self.compile_args(c.args.iter())?;
                self.emit(Op::CallFn { dst, func: idx, base, argc });
                return Ok(());
            }
        }
        // Everything else, resolved by the VM through the bridge dispatch.
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        let p = self.add_path(segs, coerce);
        let base = self.compile_args(c.args.iter())?;
        self.emit(Op::CallPath { dst, path: p, base, argc });
        Ok(())
    }

    pub(super) fn compile_method(&mut self, dst: Reg, m: &syn::ExprMethodCall) -> Result<()> {
        let recv = self.compile_expr(&m.receiver)?;
        let base = self.compile_args(m.args.iter())?;
        let name = self.add_name(m.method.to_string());
        self.emit(Op::Method { dst, recv, name, base, argc: m.args.len() as u16 });
        // Methods that fill a `&mut` argument, like read_line, write the new
        // value into the arg window. The window slot is only a copy of the
        // variable, so move the result back into the variable register.
        for (i, arg) in m.args.iter().enumerate() {
            if let Expr::Reference(r) = arg
                && r.mutability.is_some()
                && let Expr::Path(p) = &*r.expr
                && p.path.segments.len() == 1
                && p.qself.is_none()
                && let NameLoc::Local(reg) = self.resolve(&p.path.segments[0].ident.to_string())
            {
                self.emit(Op::Move { dst: reg, src: base + i as u16 });
            }
        }
        Ok(())
    }

    pub(super) fn compile_closure(&mut self, dst: Reg, c: &syn::ExprClosure) -> Result<()> {
        self.frames.push(FnState::new("<closure>".to_string()));
        let params: Vec<&Pat> = c.inputs.iter().collect();
        self.cur().num_params = params.len();
        for p in &params {
            let reg = self.alloc();
            match p {
                Pat::Ident(id) if id.subpat.is_none() => self.define(&id.ident.to_string(), reg),
                _ => self.bind_pattern_irrefutable(p, reg)?,
            }
        }
        let ret = self.alloc();
        self.compile_into(ret, &c.body)?;
        self.emit(Op::Ret { src: ret });
        let child = self.frames.pop().unwrap();
        let caps: Vec<CapSource> = child.upvalues.iter().map(|(_, s)| *s).collect();
        let chunk = Rc::new(child.into_chunk());
        let parent = self.cur();
        let child_idx = parent.children.len() as u16;
        parent.children.push(chunk);
        parent.child_caps.push(caps);
        self.emit(Op::MakeClosure { dst, child: child_idx });
        Ok(())
    }

    // -- assignment --------------------------------------------------------

    pub(super) fn compile_assign(&mut self, target: &Expr, value: &Expr) -> Result<()> {
        match target {
            Expr::Path(p) if p.path.segments.len() == 1 => {
                let name = p.path.segments[0].ident.to_string();
                match self.resolve(&name) {
                    NameLoc::Local(reg) => self.compile_into(reg, value)?,
                    _ => bail!("assignment to unknown or captured variable `{name}`"),
                }
            }
            Expr::Index(idx) => {
                let base = self.compile_expr(&idx.expr)?;
                let key = self.compile_expr(&idx.index)?;
                let val = self.compile_expr(value)?;
                self.emit(Op::SetIndex { base, key, val });
            }
            Expr::Field(f) => {
                let base = self.compile_expr(&f.base)?;
                let member = self.member_of(&f.member);
                let val = self.compile_expr(value)?;
                self.emit(Op::SetField { base, member, val });
            }
            Expr::Unary(u) if matches!(u.op, UnOp::Deref(_)) => {
                self.compile_assign(&u.expr, value)?;
            }
            Expr::Paren(p) => self.compile_assign(&p.expr, value)?,
            _ => bail!("invalid assignment target"),
        }
        Ok(())
    }

    pub(super) fn compile_compound_assign(&mut self, target: &Expr, op: BinKind, rhs: &Expr) -> Result<()> {
        // `a op= b` becomes `a = a op b`.
        match target {
            Expr::Path(p) if p.path.segments.len() == 1 => {
                let name = p.path.segments[0].ident.to_string();
                let reg = match self.resolve(&name) {
                    NameLoc::Local(reg) => reg,
                    _ => bail!("assignment to unknown or captured variable `{name}`"),
                };
                if let Some(imm) = int_literal(rhs) {
                    self.emit(Op::BinImm { dst: reg, a: reg, imm, op });
                    return Ok(());
                }
                let b = self.compile_expr(rhs)?;
                self.emit(Op::Bin { dst: reg, a: reg, b, op });
            }
            Expr::Index(idx) => {
                let base = self.compile_expr(&idx.expr)?;
                let key = self.compile_expr(&idx.index)?;
                let cur = self.alloc();
                self.emit(Op::Index { dst: cur, base, key });
                let b = self.compile_expr(rhs)?;
                let res = self.alloc();
                self.emit(Op::Bin { dst: res, a: cur, b, op });
                self.emit(Op::SetIndex { base, key, val: res });
            }
            Expr::Field(f) => {
                let base = self.compile_expr(&f.base)?;
                let member = self.member_of(&f.member);
                let cur = self.alloc();
                self.emit(Op::GetField { dst: cur, base, member });
                let b = self.compile_expr(rhs)?;
                let res = self.alloc();
                self.emit(Op::Bin { dst: res, a: cur, b, op });
                self.emit(Op::SetField { base, member, val: res });
            }
            _ => bail!("invalid compound assignment target"),
        }
        Ok(())
    }

    pub(super) fn member_of(&mut self, member: &syn::Member) -> u16 {
        match member {
            syn::Member::Named(n) => self.add_member(Member::Named(n.to_string().into())),
            syn::Member::Unnamed(i) => self.add_member(Member::Indexed(i.index as usize)),
        }
    }

    pub(super) fn compile_struct_literal(&mut self, dst: Reg, s: &syn::ExprStruct) -> Result<()> {
        let name = s.path.segments.last().map(|seg| seg.ident.to_string()).unwrap_or_default();
        // Written fields keyed by name.
        let mut written: Vec<(String, &Expr)> = Vec::new();
        for f in &s.fields {
            let key = match &f.member {
                syn::Member::Named(n) => n.to_string(),
                syn::Member::Unnamed(i) => i.index.to_string(),
            };
            written.push((key, &f.expr));
        }
        // Field order follows the declaration when the struct is known.
        // Written fields in declaration order, then any extras. A trailing
        // `..rest` fills whatever was not written.
        let order: Vec<String> = match self.ctx.structs.get(&name) {
            Some(def) => {
                let mut ordered: Vec<String> = def
                    .fields
                    .iter()
                    .filter_map(|f| f.ident.as_ref().map(|i| i.to_string()))
                    .filter(|k| written.iter().any(|(w, _)| w == k))
                    .collect();
                for (k, _) in &written {
                    if !ordered.contains(k) {
                        ordered.push(k.clone());
                    }
                }
                ordered
            }
            None => written.iter().map(|(k, _)| k.clone()).collect(),
        };
        // Reserve a packed window, then fill it, so field temporaries do not
        // break the packing.
        let has_rest = s.rest.is_some();
        let slots = order.len() + usize::from(has_rest);
        let base = self.cur().reg_top;
        for _ in 0..slots {
            self.alloc();
        }
        for (i, fname) in order.iter().enumerate() {
            let dstf = base + i as Reg;
            match written.iter().find(|(k, _)| k == fname) {
                Some((_, e)) => self.compile_into(dstf, e)?,
                None => self.emit(Op::LoadUnit { dst: dstf }),
            }
        }
        if let Some(rest) = &s.rest {
            self.compile_into(base + order.len() as Reg, rest)?;
        }
        let info = {
            let f = self.cur();
            f.struct_lits.push(StructLit {
                name: name.into(),
                fields: order.into_iter().map(Into::into).collect(),
                has_rest,
            });
            (f.struct_lits.len() - 1) as u16
        };
        self.emit(Op::MakeStruct { dst, info, base });
        Ok(())
    }

    // -- patterns ----------------------------------------------------------

    /// Register a pattern and the slot each bound name uses.
    pub(super) fn pattern_info(&mut self, pat: &Pat) -> Result<u16> {
        let mut names = Vec::new();
        collect_pattern_names(pat, &mut names);
        let mut binds = Vec::new();
        for n in names {
            let reg = self.alloc();
            self.define(&n, reg);
            binds.push((n, reg));
        }
        let f = self.cur();
        f.pats.push(PatInfo { pat: Rc::new(pat.clone()), binds });
        Ok((f.pats.len() - 1) as u16)
    }

    /// Bind an irrefutable pattern whose value already sits in `reg`.
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
                // Tuple or struct destructuring, use a test-and-bind that always
                // matches for these irrefutable shapes.
                let matched = self.alloc();
                let pidx = self.pattern_info(pat)?;
                self.emit(Op::TestBind { val: reg, pat: pidx, dst: matched });
                Ok(())
            }
        }
    }

    // -- macros ------------------------------------------------------------

}
