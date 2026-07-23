//! Calls, closures, assignment, struct literals, and patterns. Split from the compiler.

use std::rc::Rc;

use anyhow::{Result, bail};
use syn::{Expr, Lit, Pat, UnOp};

use crate::interpreter::bytecode::{
    BinKind, CapSource, DISCARD, Member, Op, PLit, PPat, PatInfo, Reg, StructLit,
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
                self.emit_mut_arg_writebacks(c.args.iter(), base);
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
                self.emit_mut_arg_writebacks(c.args.iter(), base);
                return Ok(());
            }
            // A tuple struct constructor.
            Res::Struct(canon) => vec![canon.to_string()],
            // An associated function, UFCS method, or tuple enum variant.
            Res::TypeMember(canon, rest) => {
                if let Some(variant) = self.enum_variant(&canon, &rest, |fields| {
                    matches!(fields, syn::Fields::Unnamed(fields) if fields.unnamed.len() == argc as usize)
                }) {
                    let base = self.compile_args(c.args.iter())?;
                    let info = self.add_enum_variant(variant);
                    self.emit(Op::MakeEnum {
                        dst,
                        info,
                        base,
                        count: argc,
                    });
                    return Ok(());
                }
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
            Res::External(segs) => {
                if is_transparent_new(&segs) && c.args.len() == 1 {
                    return self.compile_into(dst, &c.args[0]);
                }
                segs
            }
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
        // `v[a..b].copy_from_slice(src)` must write through to `v`. Indexing
        // with a range builds a copied temporary, so the call is compiled
        // against the base vec with the bounds as leading arguments instead.
        // An open end becomes the max sentinel the bridge clamps to the len.
        if m.method == "copy_from_slice" {
            let Expr::Index(ix) = &*m.receiver else {
                bail!("copy_from_slice is only supported on a `v[a..b]` receiver");
            };
            let Expr::Range(r) = &*ix.index else {
                bail!("copy_from_slice is only supported on a `v[a..b]` receiver");
            };
            let Some(src) = m.args.first() else {
                bail!("copy_from_slice takes the source slice");
            };
            let recv = self.compile_expr(&ix.expr)?;
            let base = self.cur().reg_top;
            for _ in 0..3 {
                self.alloc();
            }
            match &r.start {
                Some(e) => self.compile_into(base, e)?,
                None => self.emit(Op::LoadInt { dst: base, v: 0 }),
            }
            match &r.end {
                Some(e) => {
                    self.compile_into(base + 1, e)?;
                    if matches!(r.limits, syn::RangeLimits::Closed(_)) {
                        self.emit(Op::BinImm {
                            dst: base + 1,
                            a: base + 1,
                            imm: 1,
                            op: BinKind::Add,
                        });
                    }
                }
                None => self.emit(Op::LoadInt {
                    dst: base + 1,
                    v: i64::MAX,
                }),
            }
            self.compile_into(base + 2, src)?;
            let name = self.add_name("copy_from_slice".to_string());
            self.set_line(m.method.span());
            self.emit(Op::Method {
                dst,
                recv,
                name,
                base,
                argc: 3,
            });
            return Ok(());
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
        // `collect` is type driven in real Rust. The interpreter has no types,
        // so the two places the target is knowable lower to their own method
        // here, a turbofish asking for a String and a pending `let s: String`
        // annotation attached to exactly this call, see `Compiler::string_let`.
        let mut method = m.method.to_string();
        if method == "collect" {
            let turbofish_string = m.turbofish.as_ref().is_some_and(names_string);
            let let_string = matches!(self.string_let, Some(ptr) if std::ptr::eq(ptr, m));
            if turbofish_string || let_string {
                self.string_let = None;
                method = "collect_string".to_string();
            }
        }
        let name = self.add_name(method);
        // A multiline chain compiles its receiver and args first, so restamp
        // with the method's own line before the op lands, the line rustc
        // would name for this call.
        self.set_line(m.method.span());
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
        self.emit_mut_arg_writebacks(m.args.iter(), base);
        Ok(())
    }

    /// Emit a writeback for every `&mut variable` argument of a finished call.
    /// The callee worked on the arg window copy, and the VM hands the final
    /// values back into that window on return, so a move from the window slot
    /// lands the mutation in the caller's variable. Only calls whose window
    /// survives the call may use this, a `CallPath` consumes its args instead.
    fn emit_mut_arg_writebacks<'e>(&mut self, args: impl Iterator<Item = &'e Expr>, base: Reg) {
        for (i, arg) in args.enumerate() {
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
        let mut chunk = child.into_chunk(self.ctx.file.clone());
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
        let mut chunk = child.into_chunk(self.ctx.file.clone());
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
            pat: lower_pattern(pat),
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

fn is_transparent_new(segments: &[String]) -> bool {
    let Some((prefix, [receiver, method])) = segments.split_last_chunk::<2>() else {
        return false;
    };
    (prefix.is_empty() || matches!(prefix.first().map(String::as_str), Some("std" | "alloc")))
        && method == "new"
        && matches!(receiver.as_str(), "Box" | "Rc" | "Arc" | "RefCell" | "Cell")
}

fn lower_pattern(pattern: &Pat) -> PPat {
    match pattern {
        Pat::Wild(_) => PPat::Wild,
        Pat::Rest(_) => PPat::Rest,
        Pat::Ident(ident) => PPat::Ident {
            name: ident.ident.to_string(),
            sub: ident
                .subpat
                .as_ref()
                .map(|subpattern| Box::new(lower_pattern(&subpattern.1))),
        },
        Pat::Lit(literal) => lower_literal(&literal.lit),
        Pat::Paren(paren) => lower_pattern(&paren.pat),
        Pat::Reference(reference) => lower_pattern(&reference.pat),
        Pat::Type(typed) => lower_pattern(&typed.pat),
        Pat::Tuple(tuple) => PPat::Tuple(tuple.elems.iter().map(lower_pattern).collect()),
        Pat::TupleStruct(tuple) => PPat::TupleStruct {
            name: tuple
                .path
                .segments
                .last()
                .map(|segment| segment.ident.to_string()),
            elems: tuple.elems.iter().map(lower_pattern).collect(),
        },
        Pat::Path(path) => PPat::Path {
            name: path
                .path
                .segments
                .last()
                .map(|segment| segment.ident.to_string()),
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
                    (name, lower_pattern(&field.pat))
                })
                .collect(),
        },
        Pat::Or(or) => PPat::Or(or.cases.iter().map(lower_pattern).collect()),
        Pat::Slice(slice) => PPat::Slice(slice.elems.iter().map(lower_pattern).collect()),
        Pat::Range(range) => lower_range(range),
        _ => PPat::Unsupported,
    }
}

fn lower_range(range: &syn::PatRange) -> PPat {
    // Outer None means a present endpoint that is not a supported literal,
    // inner None means that side of the range is unbounded.
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

/// A literal range endpoint, including a negated number, seen through parens.
fn endpoint_lit(e: &Expr) -> Option<PLit> {
    match e {
        Expr::Lit(l) => match &l.lit {
            Lit::Int(value) => value.base10_parse().ok().map(PLit::Int),
            Lit::Float(value) => value.base10_parse().ok().map(PLit::Float),
            Lit::Char(value) => Some(PLit::Char(value.value())),
            Lit::Byte(value) => Some(PLit::Int(i64::from(value.value()))),
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

/// The `MIN` or `MAX` associated const of an integer type, as the i64 the
/// interpreter stores every integer in. Bounds outside i64, the u64 and u128
/// maxima, clamp to i64's range, which acts as unbounded for stored values.
fn int_type_bound(ty: &str, which: &str) -> Option<i64> {
    let (lo, hi) = match ty {
        "i8" => (i64::from(i8::MIN), i64::from(i8::MAX)),
        "i16" => (i64::from(i16::MIN), i64::from(i16::MAX)),
        "i32" => (i64::from(i32::MIN), i64::from(i32::MAX)),
        "i64" | "isize" | "i128" => (i64::MIN, i64::MAX),
        "u8" => (0, i64::from(u8::MAX)),
        "u16" => (0, i64::from(u16::MAX)),
        "u32" => (0, i64::from(u32::MAX)),
        "u64" | "usize" | "u128" => (0, i64::MAX),
        _ => return None,
    };
    match which {
        "MIN" => Some(lo),
        "MAX" => Some(hi),
        _ => None,
    }
}

fn lower_literal(literal: &Lit) -> PPat {
    match literal {
        Lit::Int(value) => value
            .base10_parse()
            .map(|value| PPat::Lit(PLit::Int(value)))
            .unwrap_or(PPat::Unsupported),
        Lit::Float(value) => value
            .base10_parse()
            .map(|value| PPat::Lit(PLit::Float(value)))
            .unwrap_or(PPat::Unsupported),
        Lit::Bool(value) => PPat::Lit(PLit::Bool(value.value)),
        Lit::Str(value) => PPat::Lit(PLit::Str(value.value())),
        Lit::Char(value) => PPat::Lit(PLit::Char(value.value())),
        Lit::Byte(value) => PPat::Lit(PLit::Int(i64::from(value.value()))),
        _ => PPat::Unsupported,
    }
}

/// Whether a call path names tokio's `spawn`, either `tokio::spawn` or
/// `tokio::task::spawn`.
fn is_tokio_spawn(path: &syn::Path) -> bool {
    let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
    segs.last().map(String::as_str) == Some("spawn") && segs.iter().any(|s| s == "tokio")
}

/// Whether a turbofish asks for a `String`, as in `collect::<String>()`.
fn names_string(tf: &syn::AngleBracketedGenericArguments) -> bool {
    tf.args.iter().any(|arg| {
        matches!(arg, syn::GenericArgument::Type(syn::Type::Path(p))
            if p.path.segments.last().is_some_and(|s| s.ident == "String"))
    })
}
