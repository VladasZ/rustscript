//! Calls, closures, assignment, struct literals and patterns.

use std::sync::Arc;

use anyhow::{Result, bail};
use syn::Expr;

use crate::interpreter::bytecode::{Const, DefaultIr, EnumVariant, Op, PathId, PathRef, Reg};
use crate::interpreter::numeric::IntWidth;
use crate::interpreter::typeir::CastIr;

use super::place;
use super::{Compiler, NameLoc, Res, TypeIr, first_generic_type, idx16};

impl Compiler<'_> {
    /// The window is reserved first so the temporaries of an argument can't break the packing.
    pub(super) fn compile_args<'e>(&mut self, args: impl Iterator<Item = &'e Expr>) -> Result<Reg> {
        let list: Vec<&Expr> = args.collect();
        let base = self.cur().reg_top;
        for _ in 0..list.len() {
            self.alloc();
        }
        for (i, a) in list.iter().enumerate() {
            self.compile_into(base + idx16(i), a)?;
            self.emit_arg_move_out(a);
        }
        Ok(base)
    }

    /// A by value argument is a move. With `Drop` impls the binding register clears after the
    /// copy, so the guard drops where the move sent it.
    pub(super) fn emit_arg_move_out(&mut self, arg: &Expr) {
        if !self.ctx.has_drop {
            return;
        }
        let Some(name) = place::single_path_name(arg) else {
            return;
        };
        if self.cur().aliases.contains_key(&name) {
            return;
        }
        if let NameLoc::Local(reg) = self.resolve(&name)
            // a borrow parameter forwards a reference, so the caller keeps its handle
            && !self.cur().borrow_params.contains(&reg)
        {
            self.emit(Op::MoveOut { src: reg });
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
                    return self.compile_spawn(dst, &block.block);
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
            return self.default_calls.remove(&std::ptr::from_ref(c));
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
            EmptyKind::Map => self.emit(Op::MakeMap { dst, set: false }),
            EmptyKind::Set => self.emit(Op::MakeMap { dst, set: true }),
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
        // `drop(x)` moves `x` out, so its register clears and the callee sees the last holder
        let cleared = if path.id == PathId::Drop && c.args.len() == 1 {
            place::single_path_name(&c.args[0]).and_then(|n| {
                let n = self.unalias(&n);
                match self.resolve(&n) {
                    NameLoc::Local(reg) => Some(reg),
                    _ => None,
                }
            })
        } else {
            None
        };
        let p = self.add_path(path);
        let base = self.compile_args(c.args.iter())?;
        if let Some(reg) = cleared {
            self.emit(Op::LoadUnit { dst: reg });
        }
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
            self.compile_into(dst, &c.args[0])?;
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

    /// Its own turbofish, a pending `let` annotation, or the enclosing signature. See
    /// `Compiler::json_let` and `Compiler::json_tails`.
    pub(super) fn call_coerce(&mut self, c: &syn::ExprCall, path: &syn::Path) -> Option<TypeIr> {
        let coerce = path
            .segments
            .last()
            .and_then(first_generic_type)
            .map(|t| self.lower_ir(t));
        match coerce {
            Some(ty) => Some(ty),
            None => match &self.json_let {
                Some((ptr, ty)) if std::ptr::eq(*ptr, c) => {
                    let ty = ty.clone();
                    self.json_let = None;
                    Some(ty)
                }
                _ => self.json_tails.get(&std::ptr::from_ref(c)).cloned(),
            },
        }
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
    Map,
    Set,
}

pub(super) fn empty_container(id: PathId) -> Option<EmptyKind> {
    Some(match id {
        PathId::VecNew | PathId::VecWithCapacity => EmptyKind::Vec,
        PathId::StringNew | PathId::StringWithCapacity => EmptyKind::Str,
        PathId::HashMapNew | PathId::HashMapWithCapacity | PathId::BTreeMapNew => EmptyKind::Map,
        PathId::HashSetNew | PathId::HashSetWithCapacity | PathId::BTreeSetNew => EmptyKind::Set,
        _ => return None,
    })
}
