//! Calls, closures, assignment, struct literals, and patterns. Split from the compiler.

use std::rc::Rc;

use anyhow::{Result, bail};
use syn::{Expr, Pat, UnOp};

use crate::interpreter::bytecode::{
    BinKind, CapSource, DISCARD, Member, Op, PatInfo, Reg, StructLit,
};
use crate::interpreter::json_bridge::serde_rename;
use crate::interpreter::value::StructShape;

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

    /// Record the turbofish type args on a call path, e.g. the `AppList` in
    /// `get_json::<AppList>(..)`, returning an index into the current chunk's
    /// `call_type_args` table, or `u32::MAX` when there are none.
    fn record_call_type_args(&mut self, path: &syn::Path) -> u32 {
        let Some(seg) = path.segments.last() else {
            return u32::MAX;
        };
        let syn::PathArguments::AngleBracketed(ab) = &seg.arguments else {
            return u32::MAX;
        };
        let types: Vec<Rc<syn::Type>> = ab
            .args
            .iter()
            .filter_map(|a| match a {
                syn::GenericArgument::Type(t) => Some(Rc::new(t.clone())),
                _ => None,
            })
            .collect();
        if types.is_empty() {
            return u32::MAX;
        }
        let table = &mut self.cur().call_type_args;
        table.push(Rc::from(types.into_boxed_slice()));
        (table.len() - 1) as u32
    }

    pub(super) fn compile_call(&mut self, dst: Reg, c: &syn::ExprCall) -> Result<()> {
        let path = match &*c.func {
            Expr::Path(p) => &p.path,
            _ => bail!("cannot call this kind of expression"),
        };
        // tokio::spawn(async { .. }) lowers to a Spawn op carrying the async
        // block as a child chunk, so the task runs on its own worker thread.
        if self.ctx.async_mode && is_tokio_spawn(path) {
            match c.args.first() {
                Some(Expr::Async(block)) if c.args.len() == 1 => {
                    return self.compile_spawn(dst, &block.block);
                }
                _ => bail!("tokio::spawn needs an async block in this interpreter"),
            }
        }
        let coerce = path
            .segments
            .last()
            .and_then(first_generic_type)
            .map(|t| Rc::new(t.clone()));
        // A pending `let` annotation attaches to exactly this call, see
        // `Compiler::json_let`.
        let coerce = match coerce {
            Some(ty) => Some(ty),
            None => match &self.json_let {
                Some((ptr, ty)) if std::ptr::eq(*ptr, c) => {
                    let ty = ty.clone();
                    self.json_let = None;
                    Some(ty)
                }
                _ => None,
            },
        };
        let argc = c.args.len() as u16;

        if path.segments.len() == 1 {
            let name = path.segments[0].ident.to_string();
            // A local closure value called directly.
            if let NameLoc::Local(reg) = self.resolve(&name) {
                let base = self.compile_args(c.args.iter())?;
                self.emit(Op::CallValue {
                    dst,
                    callee: reg,
                    base,
                    argc,
                });
                return Ok(());
            }
        }
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        let resolved = match self.resolve_path_res(&segs) {
            Ok(r) => r,
            Err(_) => Res::External(segs.clone()),
        };
        let path_segs = match resolved {
            // A known function, called directly by id. Turbofish type args are
            // recorded so the callee can bind them to its generic parameters.
            Res::Fn(idx) => {
                let targ = self.record_call_type_args(path);
                let base = self.compile_args(c.args.iter())?;
                self.emit(Op::CallFn {
                    dst,
                    func: idx,
                    base,
                    argc,
                    targ,
                });
                return Ok(());
            }
            // A tuple struct constructor.
            Res::Struct(canon) => vec![canon.to_string()],
            // An associated function, UFCS method, or tuple enum variant.
            Res::TypeMember(canon, rest) => {
                let mut segs = vec![canon.to_string()];
                segs.extend(rest);
                segs
            }
            // A tuple struct built through a type alias, `type P = Point; P(..)`.
            Res::Alias(m, target) => {
                let aliased = match &*target {
                    syn::Type::Path(p) => self.ctx.resolver.resolve_struct_key(m, &p.path),
                    _ => None,
                };
                match aliased {
                    Some(canon) => vec![canon.to_string()],
                    None => bail!("cannot call `{}`", segs.join("::")),
                }
            }
            Res::Enum(_) | Res::Module | Res::Const(_) => {
                bail!("cannot call `{}`", segs.join("::"))
            }
            // Everything else, resolved by the VM through the bridge dispatch.
            Res::External(segs) => segs,
        };
        let p = self.add_path(path_segs, coerce);
        let base = self.compile_args(c.args.iter())?;
        self.emit(Op::CallPath {
            dst,
            path: p,
            base,
            argc,
        });
        Ok(())
    }

    pub(super) fn compile_method(&mut self, dst: Reg, m: &syn::ExprMethodCall) -> Result<()> {
        // Tokio task handles yield Result in real Rust. The parallel VM already
        // turns a successful task await into its inner value, so the source-level
        // unwrap is complete once the await has been compiled.
        if self.ctx.async_mode
            && m.method == "unwrap"
            && m.args.is_empty()
            && matches!(&*m.receiver, Expr::Await(_))
        {
            return self.compile_into(dst, &m.receiver);
        }
        // Fuse `x.get(k).copied().unwrap_or(d)` into one op. The chain builds
        // and tears down an Option per call, which dominates counting loops.
        if dst != DISCARD
            && m.method == "unwrap_or"
            && m.args.len() == 1
            && let Expr::MethodCall(c) = &*m.receiver
            && (c.method == "copied" || c.method == "cloned")
            && c.args.is_empty()
            && let Expr::MethodCall(g) = &*c.receiver
            && g.method == "get"
            && g.args.len() == 1
        {
            let recv = self.compile_expr(&g.receiver)?;
            let key = self.compile_expr(&g.args[0])?;
            let default = self.compile_expr(&m.args[0])?;
            self.emit(Op::GetOrDefault {
                dst,
                recv,
                key,
                default,
            });
            return Ok(());
        }
        let recv = self.compile_expr(&m.receiver)?;
        let base = self.compile_args(m.args.iter())?;
        let name = self.add_name(m.method.to_string());
        self.emit(Op::Method {
            dst,
            recv,
            name,
            base,
            argc: m.args.len() as u16,
        });
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
                self.emit(Op::Move {
                    dst: reg,
                    src: base + i as u16,
                });
            }
        }
        Ok(())
    }

    /// Compile an `async { .. }` block from `tokio::spawn` into a zero argument
    /// child chunk and emit a Spawn op. Captures work like a closure's.
    fn compile_spawn(&mut self, dst: Reg, block: &syn::Block) -> Result<()> {
        self.frames.push(FnState::new("<task>".to_string()));
        self.cur().num_params = 0;
        let ret = self.alloc();
        self.compile_block(block, ret)?;
        self.emit(Op::Ret { src: ret });
        let child = self.frames.pop().unwrap();
        let caps: Vec<CapSource> = child.upvalues.iter().map(|(_, s)| *s).collect();
        let mut chunk = child.into_chunk();
        chunk.module = self.ctx.module as u16;
        let parent = self.cur();
        let child_idx = parent.children.len() as u16;
        parent.children.push(Rc::new(chunk));
        parent.child_caps.push(caps);
        self.emit(Op::Spawn {
            dst,
            child: child_idx,
        });
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
        let mut chunk = child.into_chunk();
        chunk.module = self.ctx.module as u16;
        let chunk = Rc::new(chunk);
        let parent = self.cur();
        let child_idx = parent.children.len() as u16;
        parent.children.push(chunk);
        parent.child_caps.push(caps);
        self.emit(Op::MakeClosure {
            dst,
            child: child_idx,
        });
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

    pub(super) fn compile_compound_assign(
        &mut self,
        target: &Expr,
        op: BinKind,
        rhs: &Expr,
    ) -> Result<()> {
        // `a op= b` becomes `a = a op b`.
        match target {
            Expr::Path(p) if p.path.segments.len() == 1 => {
                let name = p.path.segments[0].ident.to_string();
                let reg = match self.resolve(&name) {
                    NameLoc::Local(reg) => reg,
                    _ => bail!("assignment to unknown or captured variable `{name}`"),
                };
                if let Some(imm) = int_literal(rhs) {
                    self.emit(Op::BinImm {
                        dst: reg,
                        a: reg,
                        imm,
                        op,
                    });
                    return Ok(());
                }
                let b = self.compile_expr(rhs)?;
                self.emit(Op::Bin {
                    dst: reg,
                    a: reg,
                    b,
                    op,
                });
            }
            Expr::Index(idx) => {
                let base = self.compile_expr(&idx.expr)?;
                let key = self.compile_expr(&idx.index)?;
                let cur = self.alloc();
                self.emit(Op::Index {
                    dst: cur,
                    base,
                    key,
                });
                let b = self.compile_expr(rhs)?;
                let res = self.alloc();
                self.emit(Op::Bin {
                    dst: res,
                    a: cur,
                    b,
                    op,
                });
                self.emit(Op::SetIndex {
                    base,
                    key,
                    val: res,
                });
            }
            Expr::Field(f) => {
                let base = self.compile_expr(&f.base)?;
                let member = self.member_of(&f.member);
                let cur = self.alloc();
                self.emit(Op::GetField {
                    dst: cur,
                    base,
                    member,
                });
                let b = self.compile_expr(rhs)?;
                let res = self.alloc();
                self.emit(Op::Bin {
                    dst: res,
                    a: cur,
                    b,
                    op,
                });
                self.emit(Op::SetField {
                    base,
                    member,
                    val: res,
                });
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
        // A user struct resolves to its canonical name, which keys shapes,
        // methods, and coercions. Anything else, an enum struct variant for
        // example, keeps the bare last segment.
        let (name, def) = match self
            .ctx
            .resolver
            .resolve_struct_key(self.ctx.module, &s.path)
        {
            Some(canon) => {
                let def = self.ctx.resolver.structs.get(&canon).map(|d| d.ast.clone());
                (canon.to_string(), def)
            }
            None => {
                let bare = s
                    .path
                    .segments
                    .last()
                    .map(|seg| seg.ident.to_string())
                    .unwrap_or_default();
                (bare, None)
            }
        };
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
        let (order, renames): (Vec<String>, Vec<Option<Rc<str>>>) = match def {
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
                // One rename slot per ordered field, read from the struct def so
                // a serialized literal uses the same json keys as deserialize.
                let renames = ordered
                    .iter()
                    .map(|k| {
                        def.fields
                            .iter()
                            .find(|f| f.ident.as_ref().is_some_and(|i| i == k))
                            .and_then(serde_rename)
                            .map(Rc::<str>::from)
                    })
                    .collect();
                (ordered, renames)
            }
            None => (written.iter().map(|(k, _)| k.clone()).collect(), Vec::new()),
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
            let shape = StructShape::with_renames(
                name,
                order.into_iter().map(Into::into).collect(),
                renames,
            );
            let f = self.cur();
            f.struct_lits.push(StructLit { shape, has_rest });
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
        f.pats.push(PatInfo {
            pat: Rc::new(pat.clone()),
            binds,
        });
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
                self.emit(Op::TestBind {
                    val: reg,
                    pat: pidx,
                    dst: matched,
                });
                Ok(())
            }
        }
    }

    // -- macros ------------------------------------------------------------
}

/// Whether a call path names tokio's `spawn`, either `tokio::spawn` or
/// `tokio::task::spawn`.
fn is_tokio_spawn(path: &syn::Path) -> bool {
    let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
    segs.last().map(String::as_str) == Some("spawn") && segs.iter().any(|s| s == "tokio")
}
