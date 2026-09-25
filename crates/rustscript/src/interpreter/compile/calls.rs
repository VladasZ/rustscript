//! Calls, closures, assignment, struct literals and patterns.

use std::sync::Arc;

use anyhow::{Result, bail};
use syn::Expr;

use crate::interpreter::bytecode::{Const, DefaultIr, EnumVariant, Op, PathId, PathRef, Reg};
use crate::interpreter::numeric::IntWidth;
use crate::interpreter::typeir::CastIr;

use super::infer::Ty;
use super::{Compiler, NameLoc, Res, TypeIr, first_generic_type, idx16};

impl Compiler<'_> {
    /// The window is reserved first so the temporaries of an argument can't break the packing.
    /// Arguments for an op that takes the window, a call or a constructor. An owned value in
    /// it is the panic unwinder's to drop, when a later argument panics before the op runs.
    pub(super) fn compile_args<'e>(&mut self, args: impl Iterator<Item = &'e Expr>) -> Result<Reg> {
        // the tail call of a block unwinds its operands like the temporaries around them, in
        // reverse order of creation, any other call unwinds them first
        let tail = std::mem::take(&mut self.cur().tail_call);
        let list: Vec<&Expr> = args.collect();
        self.compile_window(&list, Some(tail))
    }

    /// Arguments a native method reads in place and leaves in the window, so nothing may
    /// drop them later.
    pub(super) fn compile_shared_args<'e>(
        &mut self,
        args: impl Iterator<Item = &'e Expr>,
    ) -> Result<Reg> {
        let list: Vec<&Expr> = args.collect();
        self.compile_window(&list, None)
    }

    /// The window itself. `taken` is `Some` when the op takes the owned values out of it, and
    /// says whether the call is a block's tail, see `compile_args`.
    fn compile_window(&mut self, list: &[&Expr], taken: Option<bool>) -> Result<Reg> {
        let base = self.cur().reg_top;
        for _ in 0..list.len() {
            self.alloc();
        }
        let held = self.cur().unwind_temps.len();
        for (i, a) in list.iter().enumerate() {
            let reg = base + idx16(i);
            self.compile_owned_into(reg, a)?;
            if !self.ctx.has_drop {
                continue;
            }
            // `f(&T::new())` lends a temporary that ends with the statement. A callee hands a
            // lent argument back into the window on return, so the drop finds it there.
            if let Expr::Reference(r) = a
                && self.temp_owned(&r.expr)
            {
                self.cur().owned_temps.push(reg);
            }
            if let Some(tail) = taken
                && self.arg_owned(a)
            {
                if tail {
                    self.cur().owned_temps.push(reg);
                } else {
                    self.cur().hold_operand(reg);
                }
            }
        }
        self.cur().close_operands(held);
        Ok(base)
    }

    /// Whether the window holds a value of its own for the argument. A local moves or copies
    /// in, a fresh temporary is its own, a borrow or a lent handle is not.
    pub(super) fn arg_owned(&mut self, arg: &Expr) -> bool {
        match arg {
            Expr::Paren(p) => self.arg_owned(&p.expr),
            Expr::Group(g) => self.arg_owned(&g.expr),
            Expr::Path(p) if p.path.segments.len() == 1 && p.qself.is_none() => {
                let name = p.path.segments[0].ident.to_string();
                !self.cur().aliases.contains_key(&name) && self.scrutinee_owned(arg)
            }
            // `s.t` or `make().t` moves the field out, the rest stays with its owner
            Expr::Field(f) => self.field_base_owned(&f.base),
            other => self.temp_owned(other),
        }
    }

    fn field_base_owned(&mut self, base: &Expr) -> bool {
        match base {
            Expr::Paren(p) => self.field_base_owned(&p.expr),
            Expr::Group(g) => self.field_base_owned(&g.expr),
            Expr::Field(f) => self.field_base_owned(&f.base),
            other => self.arg_owned(other),
        }
    }

    /// The `AppList` in `get_json::<AppList>(..)`. An index into `call_type_args`, or `u32::MAX`
    /// when there are none.
    pub(super) fn record_call_type_args(&mut self, path: &syn::Path) -> u32 {
        let Some(seg) = path.segments.last() else {
            return u32::MAX;
        };
        let syn::PathArguments::AngleBracketed(ab) = &seg.arguments else {
            return u32::MAX;
        };
        let mut types = Vec::new();
        for a in &ab.args {
            if let syn::GenericArgument::Type(t) = a {
                types.push(self.lower_ir(t));
            }
        }
        if types.is_empty() {
            return u32::MAX;
        }
        let table = &mut self.cur().call_type_args;
        table.push(Arc::from(types.into_boxed_slice()));
        u32::try_from(table.len() - 1).expect("type-arg table exceeds u32 indices")
    }

    pub(super) fn compile_call(&mut self, dst: Reg, c: &syn::ExprCall) -> Result<()> {
        let Expr::Path(path_expr) = &*c.func else {
            let callee = self.compile_expr(&c.func)?;
            let base = self.compile_args(c.args.iter())?;
            self.emit_borrow_takes(c.args.iter());
            self.emit(Op::CallValue {
                dst,
                callee,
                base,
                argc: idx16(c.args.len()),
            });
            self.emit_mut_arg_writebacks(c.args.iter(), base)?;
            return Ok(());
        };
        let path = &path_expr.path;
        // `tokio::spawn(async { .. })` lowers to a `Spawn` op with the block as a child chunk
        if self.ctx.async_mode && is_tokio_spawn(path) {
            match c.args.first() {
                Some(Expr::Async(block)) if c.args.len() == 1 => {
                    return self.compile_spawn(dst, &block.block, block.capture.is_some());
                }
                _ => bail!("tokio::spawn needs an async block in this interpreter"),
            }
        }
        if c.args.is_empty()
            && path
                .segments
                .last()
                .is_some_and(|seg| seg.ident == "default")
        {
            if let Some(ir) = self.default_call_ir(c, path_expr) {
                self.emit_default(dst, ir);
                return Ok(());
            }
            // a bare `Default::default()` into a type with its own `impl Default` calls it
            if path_expr.qself.is_none()
                && path.segments.len() == 2
                && path.segments[0].ident == "Default"
                && let Ty::Struct(canon) | Ty::Enum(canon) = self.types.of_node(c)
                && self
                    .ctx
                    .impl_sigs
                    .contains_key(&(canon.to_string(), "default".to_string()))
            {
                let p = self.add_path(PathRef::user(
                    vec![canon.to_string(), "default".to_string()],
                    None,
                ));
                let base = self.compile_args(std::iter::empty())?;
                self.emit(Op::CallPath {
                    dst,
                    path: p,
                    base,
                    argc: 0,
                });
                return Ok(());
            }
            // strict inference, a bare `Default::default()` must know what it builds
            if path_expr.qself.is_none()
                && path.segments.len() == 2
                && path.segments[0].ident == "Default"
                && self.types.of_node(c).is_unknown()
            {
                let line = path.segments[1].ident.span().start().line;
                bail!(
                    "unsupported: line {line} of {}, the interpreter cannot tell what type \
                     `Default::default` makes here, name it with a `let` annotation",
                    self.ctx.file
                );
            }
            // `<S>::default()` on a type with its own `fn default` is `S::default()`
            if let Some(qself) = &path_expr.qself
                && let syn::Type::Path(tp) = &*qself.ty
            {
                let mut merged = tp.path.clone();
                merged.segments.extend(path.segments.iter().cloned());
                let coerce = self.call_coerce(c, &merged);
                return self.compile_resolved_call(dst, c, &merged, coerce, 0);
            }
        }
        let coerce = self.call_coerce(c, path);
        let argc = idx16(c.args.len());

        if self.try_compile_closure_call(dst, c, path, argc)? {
            return Ok(());
        }
        self.compile_resolved_call(dst, c, path, coerce, argc)
    }

    /// `<T>::default()` names it in the qualified self, `T::default()` in the path, a bare
    /// `Default::default()` takes it from the context hint.
    pub(super) fn default_call_ir(
        &mut self,
        c: &syn::ExprCall,
        path_expr: &syn::ExprPath,
    ) -> Option<DefaultIr> {
        if let Some(qself) = &path_expr.qself {
            return self.default_ir(&qself.ty);
        }
        let path = &path_expr.path;
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        if segs.len() == 1 {
            return None;
        }
        if segs[..segs.len() - 1] == ["Default"] {
            let ty = self.types.of_node(c);
            return self.default_ir_of(&ty);
        }
        // a user `impl Default` or inherent `fn default` wins over the derive
        let owner = segs[segs.len() - 2].clone();
        let user_defined = self.ctx.impl_methods.iter().any(|(ty, name)| {
            name == "default" && (*ty == owner || super::super::resolver::bare(ty) == owner)
        });
        if user_defined {
            return None;
        }
        let prefix = syn::Path {
            leading_colon: path.leading_colon,
            segments: path.segments.iter().take(segs.len() - 1).cloned().collect(),
        };
        self.default_ir_path_pub(&prefix)
    }

    /// Builtin variants and empty container constructors build in place, so a `Vec::new()` in a loop
    /// skips the path dispatch. A `with_capacity` argument still runs.
    pub(super) fn compile_builtin_ctor(
        &mut self,
        dst: Reg,
        c: &syn::ExprCall,
        path: &PathRef,
        argc: u16,
    ) -> Result<bool> {
        if let Some((def, index)) = self.resolve_variant(&path.segs)
            && !def.is_unit(index)
        {
            let base = self.compile_args(c.args.iter())?;
            let info = self.add_enum_variant(EnumVariant {
                def,
                variant: index,
            });
            self.emit(Op::MakeEnum {
                dst,
                info,
                base,
                count: argc,
            });
            return Ok(true);
        }
        let Some(kind) = empty_container(path.id) else {
            return Ok(false);
        };
        let base = self.compile_args(c.args.iter())?;
        match kind {
            EmptyKind::Vec => self.emit(Op::MakeVec {
                dst,
                base,
                count: 0,
            }),
            EmptyKind::Str => {
                let k = self.add_const(Const::Str(Arc::from("")));
                self.emit(Op::LoadConst { dst, k });
            }
            EmptyKind::Map(sorted) => self.emit(Op::MakeMap {
                dst,
                set: false,
                sorted,
            }),
            EmptyKind::Set(sorted) => self.emit(Op::MakeMap {
                dst,
                set: true,
                sorted,
            }),
        }
        Ok(true)
    }

    /// The path resolved half of `compile_call`.
    pub(super) fn compile_resolved_call(
        &mut self,
        dst: Reg,
        c: &syn::ExprCall,
        path: &syn::Path,
        coerce: Option<TypeIr>,
        argc: u16,
    ) -> Result<()> {
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        // `Self::..` inside an impl names the impl's own type
        let resolved = match (segs.first().map(String::as_str), self.ctx.impl_type) {
            (Some("Self"), Some(ty)) if segs.len() > 1 => {
                Res::TypeMember(Arc::from(ty), segs[1..].to_vec())
            }
            _ => self.resolve_path_res(&segs)?,
        };
        let path = match resolved {
            // turbofish type args are recorded so the callee can bind them
            Res::Fn(idx) => {
                let targ = self.record_call_type_args(path);
                let base = self.compile_args(c.args.iter())?;
                self.emit_borrow_takes(c.args.iter());
                self.emit(Op::CallFn {
                    dst,
                    func: idx,
                    base,
                    argc,
                    targ,
                });
                self.emit_mut_arg_writebacks(c.args.iter(), base)?;
                return Ok(());
            }
            Res::Struct(canon) => PathRef::user(vec![canon.to_string()], coerce),
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
                // `T::from(x)` picks the impl for the type of `x` here, not by the value
                if rest.as_slice() == ["from"] && c.args.len() == 1 {
                    let source = self.types.of(&c.args[0]);
                    let path = PathRef::user(self.impl_path_for_from(&canon, &source), coerce);
                    let p = self.add_path(path);
                    let base = self.compile_args(c.args.iter())?;
                    self.emit(Op::CallPath {
                        dst,
                        path: p,
                        base,
                        argc,
                    });
                    return Ok(());
                }
                let mut segs = vec![canon.to_string()];
                segs.extend(rest);
                PathRef::user(segs, coerce)
            }
            // `type P = Point; P(..)`
            Res::Alias(m, target) => {
                let aliased = match &*target {
                    syn::Type::Path(p) => self.ctx.resolver.resolve_struct_key(m, &p.path),
                    _ => None,
                };
                match aliased {
                    Some(canon) => PathRef::user(vec![canon.to_string()], coerce),
                    None => bail!("cannot call `{}`", segs.join("::")),
                }
            }
            Res::Enum(_) | Res::Module | Res::Const(_) => {
                bail!("cannot call `{}`", segs.join("::"))
            }
            Res::External(segs) => {
                let path = self.external_path(segs, coerce);
                match self.compile_external_call(dst, c, path, argc)? {
                    Some(path) => path,
                    None => return Ok(()),
                }
            }
        };
        let p = self.add_path(path);
        let base = self.compile_args(c.args.iter())?;
        self.emit(Op::CallPath {
            dst,
            path: p,
            base,
            argc,
        });
        Ok(())
    }

    /// Gives the path back when the call still needs the VM.
    pub(super) fn compile_external_call(
        &mut self,
        dst: Reg,
        c: &syn::ExprCall,
        path: PathRef,
        argc: u16,
    ) -> Result<Option<PathRef>> {
        // Only `Box::new` is a pass through, a box is pure ownership. `Rc`, `Arc`, `RefCell`,
        // `Cell` and `Mutex` build real shared cells.
        if path.id == PathId::BoxNew && c.args.len() == 1 {
            self.compile_owned_into(dst, &c.args[0])?;
            return Ok(None);
        }
        // the mem place functions move values between places, only the compiler can express that
        if matches!(
            path.id,
            PathId::MemSwap | PathId::MemTake | PathId::MemReplace
        ) && self.compile_mem_intrinsic(dst, path.id, c)?
        {
            return Ok(None);
        }
        // numeric `T::from(x)` is the `x as T` cast op, `rustc` already proved it lossless
        if let Some(ir) = numeric_from_cast(path.id, c.args.len()) {
            let src = self.compile_expr(&c.args[0])?;
            let f = self.cur();
            f.casts.push(ir);
            let ty = idx16(f.casts.len() - 1);
            self.emit(Op::Cast { dst, src, ty });
            return Ok(None);
        }
        if self.compile_builtin_ctor(dst, c, &path, argc)? {
            return Ok(None);
        }
        Ok(Some(path))
    }

    /// The type a parse lands in, from its own turbofish or the inferred result of the call.
    pub(super) fn call_coerce(&mut self, c: &syn::ExprCall, path: &syn::Path) -> Option<TypeIr> {
        if let Some(ty) = path.segments.last().and_then(first_generic_type) {
            return Some(self.lower_ir(ty));
        }
        let parses = path.segments.last().is_some_and(|s| {
            matches!(
                s.ident.to_string().as_str(),
                "from_str" | "from_value" | "from_slice" | "from_reader"
            )
        });
        if !parses {
            return None;
        }
        let target = self.types.of_node(c).payload();
        let ir = Self::type_ir_of(&target);
        ir.is_active().then_some(ir)
    }

    /// True when the call was emitted here.
    pub(super) fn try_compile_closure_call(
        &mut self,
        dst: Reg,
        c: &syn::ExprCall,
        path: &syn::Path,
        argc: u16,
    ) -> Result<bool> {
        if path.segments.len() != 1 {
            return Ok(false);
        }
        let name = path.segments[0].ident.to_string();
        let callee = match self.resolve(&name) {
            NameLoc::Local(reg) => Some(reg),
            NameLoc::Cell(cell) => {
                let reg = self.alloc();
                self.emit(Op::LoadCell { dst: reg, cell });
                Some(reg)
            }
            NameLoc::Upvalue(idx) => {
                let reg = self.alloc();
                self.emit(Op::LoadUpvalue { dst: reg, idx });
                Some(reg)
            }
            NameLoc::None => None,
        };
        let Some(callee) = callee else {
            return Ok(false);
        };
        let base = self.compile_args(c.args.iter())?;
        self.emit_borrow_takes(c.args.iter());
        self.emit(Op::CallValue {
            dst,
            callee,
            base,
            argc,
        });
        self.emit_mut_arg_writebacks(c.args.iter(), base)?;
        Ok(true)
    }
}

/// The cast target of a numeric `T::from(x)` call.
pub(super) fn numeric_from_cast(id: PathId, argc: usize) -> Option<CastIr> {
    if argc != 1 {
        return None;
    }
    match id {
        PathId::F64From => Some(CastIr::F64),
        PathId::F32From => Some(CastIr::F32),
        PathId::I8From
        | PathId::I16From
        | PathId::I32From
        | PathId::I64From
        | PathId::I128From
        | PathId::IsizeFrom
        | PathId::U8From
        | PathId::U16From
        | PathId::U32From
        | PathId::U64From
        | PathId::U128From
        | PathId::UsizeFrom => IntWidth::parse(id.namespace()).map(CastIr::Int),
        _ => None,
    }
}

/// `tokio::spawn` or `tokio::task::spawn`
pub(super) fn is_tokio_spawn(path: &syn::Path) -> bool {
    let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
    segs.last().map(String::as_str) == Some("spawn") && segs.iter().any(|s| s == "tokio")
}

/// 1 op, so a `Vec::new()` in a loop skips the path dispatch.
pub(super) enum EmptyKind {
    Vec,
    Str,
    /// the flag is a `BTreeMap`
    Map(bool),
    /// the flag is a `BTreeSet`
    Set(bool),
}

pub(super) fn empty_container(id: PathId) -> Option<EmptyKind> {
    Some(match id {
        PathId::VecNew
        | PathId::VecWithCapacity
        | PathId::VecDequeNew
        | PathId::VecDequeWithCapacity => EmptyKind::Vec,
        PathId::StringNew | PathId::StringWithCapacity => EmptyKind::Str,
        PathId::HashMapNew | PathId::HashMapWithCapacity | PathId::MapNew => EmptyKind::Map(false),
        PathId::BTreeMapNew => EmptyKind::Map(true),
        PathId::HashSetNew | PathId::HashSetWithCapacity => EmptyKind::Set(false),
        PathId::BTreeSetNew => EmptyKind::Set(true),
        _ => return None,
    })
}
