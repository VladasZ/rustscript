//! Closures, spawned blocks and the `&mut` argument writebacks.

use std::sync::Arc;

use anyhow::Result;
use syn::{Expr, Pat};

use crate::interpreter::bytecode::{CapSource, Op, Reg};

use super::infer::Ty;
use super::place::{self, copies};
use super::support::chain_lends;
use super::walks::{block_tail, first_arm, then_tail, unparen};
use super::{Compiler, FnState, NameLoc, captures, derives_copy, idx16, numeric_annotation};

impl Compiler<'_> {
    /// The callee worked on the arg window copy and the VM hands it back on return. Only for calls
    /// whose window survives, a `CallPath` consumes its args.
    pub(super) fn emit_mut_arg_writebacks<'e>(
        &mut self,
        args: impl Iterator<Item = &'e Expr>,
        base: Reg,
    ) -> Result<()> {
        self.emit_mut_arg_writebacks_skipping(args, base, &[])
    }

    /// `emit_mut_arg_writebacks` without the arguments `skip` marks.
    pub(super) fn emit_mut_arg_writebacks_skipping<'e>(
        &mut self,
        args: impl Iterator<Item = &'e Expr>,
        base: Reg,
        skip: &[bool],
    ) -> Result<()> {
        self.emit_arg_writebacks(args, base, skip, true)
    }

    /// The writebacks after a method call. Nothing emptied the registers of its borrowed
    /// arguments, see `emit_borrow_takes`, so their window slots are only cleared. A native
    /// that keeps its argument, `held.push(&text)`, took the slot, and a move back would
    /// store unit over the caller's value.
    pub(super) fn emit_method_arg_writebacks<'e>(
        &mut self,
        args: impl Iterator<Item = &'e Expr>,
        base: Reg,
    ) -> Result<()> {
        self.emit_arg_writebacks(args, base, &[], false)
    }

    /// `lent` says the call emptied the registers of its borrowed arguments, so each one
    /// comes back out of its window slot.
    fn emit_arg_writebacks<'e>(
        &mut self,
        args: impl Iterator<Item = &'e Expr>,
        base: Reg,
        skip: &[bool],
        lent: bool,
    ) -> Result<()> {
        for (i, arg) in args.enumerate() {
            if skip.get(i).copied().unwrap_or(false) {
                continue;
            }
            if let Expr::Reference(r) = unparen(arg)
                && r.mutability.is_some()
                && let Expr::Path(p) = &*r.expr
                && p.path.segments.len() == 1
                && p.qself.is_none()
            {
                let name = p.path.segments[0].ident.to_string();
                let location = self.resolve_for_write(&name);
                // the value comes back from a lend, the callee already dropped what it replaced,
                // so the store must find nothing to drop
                match location {
                    NameLoc::Cell(cell) => self.emit(Op::ClearCell { cell }),
                    NameLoc::Upvalue(idx) => self.emit(Op::ClearUpvalue { idx }),
                    NameLoc::Local(_) | NameLoc::None => {}
                }
                self.emit_name_store(location, base + idx16(i), &name)?;
                continue;
            }
            // the window slot clears after the move, a stale copy would inflate `Rc::strong_count`
            if let Some(reg) = self.borrowed_local(arg) {
                if lent {
                    self.emit(Op::Move {
                        dst: reg,
                        src: base + idx16(i),
                    });
                }
                self.emit(Op::LoadUnit {
                    dst: base + idx16(i),
                });
            }
        }
        Ok(())
    }

    /// The arguments that forward a reference the caller holds into a parameter the callee
    /// takes by value, `one(p)` into `fn one<T>(a: T)`. The callee owns that parameter and may
    /// move it on, into its result, so the caller lends nothing and keeps its own handle.
    pub(super) fn forwards_by_value(&mut self, func: u32, call: &syn::ExprCall) -> Vec<bool> {
        let by_value = self.by_value_params(func);
        call.args
            .iter()
            .enumerate()
            .map(|(i, arg)| {
                by_value.get(i).copied().unwrap_or(false)
                    && !matches!(unparen(arg), Expr::Reference(_))
                    && self.borrowed_local(arg).is_some()
            })
            .collect()
    }

    /// Which parameters of the script function are taken by value, a `T` and not a `&T`.
    fn by_value_params(&self, func: u32) -> Vec<bool> {
        match self.fn_signature(func) {
            Some(sig) => sig
                .inputs
                .iter()
                .map(|input| {
                    matches!(input, syn::FnArg::Typed(t)
                        if !matches!(&*t.ty, syn::Type::Reference(_)))
                })
                .collect(),
            None => Vec::new(),
        }
    }

    /// The arguments that are shared references going into a by value parameter, which only
    /// a generic one accepts. Only a program with a `Drop` impl can tell who owns the value
    /// behind one, so the others keep the plain handle.
    pub(super) fn refs_by_value(&mut self, func: u32, call: &syn::ExprCall) -> Vec<bool> {
        if !self.ctx.has_drop {
            return Vec::new();
        }
        let by_value = self.by_value_params(func);
        let generic = self.generic_params(func);
        call.args
            .iter()
            .enumerate()
            .map(|(i, arg)| {
                by_value.get(i).copied().unwrap_or(false)
                    && self.is_shared_ref(arg, generic.get(i).copied().unwrap_or(false))
            })
            .collect()
    }

    /// Which parameters of the script function have a bare type parameter or an `impl Trait`
    /// as their type, the only by value ones a reference fits.
    fn generic_params(&self, func: u32) -> Vec<bool> {
        let Some(sig) = self.fn_signature(func) else {
            return Vec::new();
        };
        let any_type =
            |ty: &syn::Type| matches!(ty, syn::Type::ImplTrait(_)) || type_param(sig, ty).is_some();
        sig.inputs
            .iter()
            .map(|input| matches!(input, syn::FnArg::Typed(t) if any_type(&t.ty)))
            .collect()
    }

    /// Whether a call of a script function hands out storage someone else owns, so nothing
    /// drops its result here. It returns a reference, or a bare type parameter that an
    /// argument fills with a reference, `pick(&a, &b)`.
    pub(super) fn call_lends(&self, call: &syn::ExprCall) -> bool {
        let Expr::Path(path) = &*call.func else {
            return false;
        };
        let Some(sig) = self
            .script_fn(&path.path)
            .and_then(|func| self.fn_signature(func))
        else {
            return false;
        };
        let syn::ReturnType::Type(_, ret) = &sig.output else {
            return false;
        };
        if matches!(&**ret, syn::Type::Reference(_)) {
            return true;
        }
        let Some(param) = type_param(sig, ret) else {
            return false;
        };
        self.ctx.has_drop
            && sig.inputs.iter().zip(&call.args).any(|(input, arg)| {
                matches!(input, syn::FnArg::Typed(t) if type_param(sig, &t.ty) == Some(param))
                    && self.is_shared_ref(arg, true)
            })
    }

    /// Whether the expression is a shared reference by its form. `&x`, a cast to a reference
    /// like `&[] as &[T]`, a borrowing accessor, a reference the frame holds, and an `if`, a
    /// `match` or a block that hands one of these out. `generic` says the place it goes into
    /// takes any type, a type parameter or a `let`.
    pub(super) fn is_shared_ref(&self, expr: &Expr, generic: bool) -> bool {
        match unparen(expr) {
            Expr::Reference(r) => r.mutability.is_none(),
            Expr::Cast(c) => {
                matches!(&*c.ty, syn::Type::Reference(r) if r.mutability.is_none())
            }
            Expr::MethodCall(m)
                if m.args.is_empty()
                    && matches!(
                        m.method.to_string().as_str(),
                        "as_slice" | "as_str" | "as_ref"
                    ) =>
            {
                true
            }
            Expr::If(i) => then_tail(i).is_some_and(|e| self.is_shared_ref(e, generic)),
            Expr::Match(m) => first_arm(m).is_some_and(|e| self.is_shared_ref(e, generic)),
            Expr::Block(b) if b.label.is_none() => {
                block_tail(&b.block).is_some_and(|e| self.is_shared_ref(e, generic))
            }
            // a binding that holds a reference and an accessor that hands one out fit a
            // place of any type, where the callee would drop them at its end
            other => {
                self.names_ref_param(other)
                    || (generic && (self.names_ref(other) || self.lends_item(other)))
            }
        }
    }

    /// The accessors `first`, `last` and `get` hand out a reference to an item of the
    /// collection, an `Option<&T>`, and so does a terminal like `next` or `max_by_key` over
    /// a chain that starts at `iter` and only passes its items on. An item with nothing to
    /// drop stays a plain value.
    fn lends_item(&self, expr: &Expr) -> bool {
        let Expr::MethodCall(m) = expr else {
            return false;
        };
        let name = m.method.to_string();
        let lends = match self.types.of(&m.receiver) {
            Ty::Vec(_) | Ty::Map(..) => matches!(name.as_str(), "first" | "last" | "get"),
            Ty::Iter(_) => {
                matches!(
                    name.as_str(),
                    "next"
                        | "next_back"
                        | "last"
                        | "nth"
                        | "max"
                        | "min"
                        | "max_by"
                        | "min_by"
                        | "max_by_key"
                        | "min_by_key"
                        | "find"
                        | "reduce"
                ) && chain_lends(&m.receiver)
            }
            _ => false,
        };
        lends && self.has_parts_to_drop(expr)
    }

    /// A mutable local lives in a cell and a `&mut` alias points elsewhere, so both stay out.
    pub(super) fn borrowed_local(&mut self, arg: &Expr) -> Option<Reg> {
        let (name, forwarded) = match unparen(arg) {
            Expr::Reference(r) => (place::single_path_name(&r.expr)?, false),
            other => (place::single_path_name(other)?, true),
        };
        if self.cur().aliases.contains_key(&name) {
            return None;
        }
        let NameLoc::Local(reg) = self.resolve(&name) else {
            return None;
        };
        if forwarded && !self.cur().borrow_params.contains(&reg) {
            return None;
        }
        Some(reg)
    }

    /// The callee then holds the only live handle, so `Rc::strong_count` reads the same at any
    /// depth. The writebacks restore the registers, and a panic in the callee restores them
    /// through `Chunk::lent_writebacks`. Always right before the call op. An argument `kept`
    /// marks stays with the caller, see `forwards_by_value`.
    pub(super) fn emit_borrow_takes<'e>(
        &mut self,
        args: impl Iterator<Item = &'e Expr>,
        kept: &[bool],
    ) {
        let regs: Vec<(usize, Reg)> = args
            .enumerate()
            .filter(|(i, _)| !kept.get(*i).copied().unwrap_or(false))
            .filter_map(|(i, arg)| self.borrowed_local(arg).map(|reg| (i, reg)))
            .collect();
        for &(_, reg) in &regs {
            self.emit(Op::LoadUnit { dst: reg });
        }
        if self.ctx.has_drop && !regs.is_empty() {
            let call_ip = u32::try_from(self.here()).unwrap_or(u32::MAX);
            let f = self.cur();
            for (i, reg) in regs {
                f.lent_writebacks.push((call_ip, idx16(i), reg));
            }
        }
    }

    /// Captures work like in a closure, `async move` like a `move` closure.
    pub(super) fn compile_spawn(
        &mut self,
        dst: Reg,
        block: &syn::Block,
        moves: bool,
    ) -> Result<()> {
        self.frames.push(FnState::new("<task>".to_string()));
        self.cur().num_params = 0;
        self.scan_captures(block);
        let ret = self.alloc();
        self.compile_block(block, ret)?;
        self.emit(Op::Ret { src: ret });
        let child = self
            .frames
            .pop()
            .expect("the closure frame was just pushed");
        let caps: Vec<CapSource> = child.upvalues.iter().map(|(_, s)| *s).collect();
        let partial = vec![false; caps.len()];
        let by_value = vec![false; caps.len()];
        let mut chunk = child.into_chunk(self.ctx.file.clone())?;
        chunk.module = idx16(self.ctx.module);
        chunk.moves = moves;
        let parent = self.cur();
        let child_idx = idx16(parent.children.len());
        parent.children.push(Arc::new(chunk));
        parent.child_caps.push(caps);
        parent.child_partial.push(partial);
        parent.child_by_value.push(by_value);
        self.emit(Op::Spawn {
            dst,
            child: child_idx,
        });
        Ok(())
    }

    /// Whether no value of the type is `Copy`, so a by value use of it is always a move. A
    /// bridge type is left out, the pass does not know which of them copy.
    pub(super) fn never_copies(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Str | Ty::Vec(_) | Ty::Set(..) | Ty::Map(..) => true,
            Ty::Struct(canon) => self.is_non_copy_name(canon),
            Ty::Enum(canon) => self
                .ctx
                .resolver
                .enums
                .get(canon)
                .is_some_and(|e| !derives_copy(&e.attrs)),
            Ty::Option(inner) => self.never_copies(inner),
            Ty::Result(ok, err) => self.never_copies(ok) || self.never_copies(err),
            Ty::Tuple(items) => items.iter().any(|item| self.never_copies(item)),
            _ => false,
        }
    }

    /// Whether every value of the type is `Copy`, a `Copy` struct or enum included.
    fn always_copies(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Struct(canon) => self
                .ctx
                .resolver
                .structs
                .get(canon.as_ref())
                .is_some_and(|def| derives_copy(&def.ast.attrs)),
            Ty::Enum(canon) => self
                .ctx
                .resolver
                .enums
                .get(canon)
                .is_some_and(|e| derives_copy(&e.attrs)),
            Ty::Option(inner) => self.always_copies(inner),
            Ty::Tuple(items) => items.iter().all(|item| self.always_copies(item)),
            other => copies(other),
        }
    }

    /// Whether the type is `Copy` and its value keeps storage of its own, a tuple, an
    /// `Option` or a `Copy` struct. A second handle to such a value must be a copy. A scalar
    /// has no storage to share.
    pub(super) fn copies_storage(&self, ty: &Ty) -> bool {
        matches!(
            ty,
            Ty::Option(_) | Ty::Tuple(_) | Ty::Struct(_) | Ty::Enum(_)
        ) && self.always_copies(ty)
    }

    /// An owned read of a binding that lives in a capture cell. A `Copy` value leaves the
    /// cell full for a closure that reads it later. A tuple or an `Option` has storage, and
    /// the handle out of the cell shares it, so the read copies. A callee that drops its copy
    /// would empty the cell too. Any other value moves out when this is its last use.
    pub(super) fn own_cell_read(&mut self, dst: Reg, cell: Reg, expr: &Expr) {
        self.emit(Op::LoadCell { dst, cell });
        let ty = self.types.of(expr);
        if !copies(&ty) {
            self.emit(Op::Own {
                dst,
                src: dst,
                root: cell,
            });
        } else if self.copies_storage(&ty) {
            self.emit(Op::Copy { dst, src: dst });
        }
    }

    /// Whether the frame owns the value in this register. The inferred type of a reference is
    /// its referent, so the type alone can't tell `v` from `&v`. A reference local or
    /// parameter only forwards a handle, and a parameter of a closure is owned or lent as its
    /// caller decides, so neither is the frame's to give away.
    fn frame_owns(frame: &FnState, reg: Reg) -> bool {
        let closure_param = frame.name == "<closure>" && (reg as usize) < frame.num_params;
        !frame.shares_only(reg) && !closure_param
    }

    /// `frame_owns` for a capture of the closure just compiled, asked on its parent.
    fn owns_local(&self, source: CapSource) -> bool {
        let (CapSource::Local(reg) | CapSource::MutableLocal(reg)) = source else {
            return false;
        };
        self.frames
            .last()
            .is_some_and(|frame| Self::frame_owns(frame, reg))
    }

    /// `frame_owns` for a name the closure being compiled captures from the frame around it.
    pub(super) fn parent_owns(&self, name: &str) -> bool {
        let Some(parent) = self.frames.len().checked_sub(2).map(|i| &self.frames[i]) else {
            return false;
        };
        parent
            .local_reg(name)
            .is_some_and(|reg| Self::frame_owns(parent, reg))
    }

    pub(super) fn compile_closure(&mut self, dst: Reg, c: &syn::ExprClosure) -> Result<()> {
        self.frames.push(FnState::new("<closure>".to_string()));
        let params: Vec<&Pat> = c.inputs.iter().collect();
        self.cur().num_params = params.len();
        // a pattern param binds more registers, so every param slot is claimed before any binding
        let regs: Vec<Reg> = params.iter().map(|_| self.alloc()).collect();
        self.scan_captures_expr(&c.body);
        for (p, reg) in params.iter().zip(regs) {
            // a reference param shares the caller's storage, so it never splits
            if let Pat::Type(t) = p
                && matches!(&*t.ty, syn::Type::Reference(_))
            {
                self.cur().borrow_params.insert(reg);
            }
            // a `mut` parameter owns a copy unless its type rules `Copy` out, see `compile_fn`
            let (binding, annotation) = match p {
                Pat::Type(t) => (&*t.pat, Some(&*t.ty)),
                other => (*other, None),
            };
            if let Pat::Ident(id) = binding
                && id.mutability.is_some()
                && !matches!(annotation, Some(syn::Type::Reference(_)))
                && !self.is_non_copy_annotation(annotation)
            {
                self.emit(Op::Copy { dst: reg, src: reg });
            }
            // a numeric annotation retags the value like a fn param
            if let Pat::Type(t) = p
                && numeric_annotation(&t.ty).is_some()
            {
                let idx = self.add_cast(&t.ty);
                self.emit(Op::Cast {
                    dst: reg,
                    src: reg,
                    ty: idx,
                });
            }
            // the binding comes last, a capture cell made at it must hold the retagged copy
            match p {
                Pat::Ident(id) if id.subpat.is_none() => self.define(&id.ident.to_string(), reg),
                _ => {
                    self.bind_pattern_irrefutable(p, reg)?;
                    self.hold_wild_param(p, reg);
                }
            }
        }
        if let syn::ReturnType::Type(_, ty) = &c.output
            && numeric_annotation(ty).is_some()
        {
            let idx = self.add_cast(ty);
            self.cur().ret_cast = Some(idx);
        }
        let ret = self.alloc();
        // a bare call body is the closure's tail call, so its owned operands unwind after the
        // temporaries of its arguments like a block's tail, see `compile_block_inner`
        self.mark_root_call(&c.body);
        // the body value leaves the closure, so a returned parameter moves out before the
        // parameters drop
        self.compile_owned_into(ret, &c.body)?;
        self.cur().next_call.root = false;
        self.drop_temps(0, Some(ret));
        self.release_guard_temps(0, Some(ret));
        if let Some(idx) = self.cur().ret_cast {
            self.emit(Op::Cast {
                dst: ret,
                src: ret,
                ty: idx,
            });
        }
        // the parameters drop before the closure returns when the call handed them over
        self.emit_param_drops();
        self.emit(Op::Ret { src: ret });
        let child = self
            .frames
            .pop()
            .expect("the closure frame was just pushed");
        let caps: Vec<CapSource> = child.upvalues.iter().map(|(_, s)| *s).collect();
        // a `move` closure that reads only fields that copy takes those fields, not the value
        let partial: Vec<bool> = child
            .upvalues
            .iter()
            .map(|(name, _)| {
                captures::captures_only_copy_fields(&c.body, name, &|e| copies(&self.types.of(e)))
            })
            .collect();
        let by_value: Vec<bool> = child
            .upvalues
            .iter()
            .map(|(name, source)| {
                self.owns_local(*source)
                    && captures::moves_out(&c.body, name, &|e| {
                        !self.never_copies(&self.types.of(e))
                    })
            })
            .collect();
        let mut chunk = child.into_chunk(self.ctx.file.clone())?;
        chunk.module = idx16(self.ctx.module);
        chunk.moves = c.capture.is_some();
        let chunk = Arc::new(chunk);
        let parent = self.cur();
        let child_idx = idx16(parent.children.len());
        parent.children.push(chunk);
        parent.child_caps.push(caps);
        parent.child_partial.push(partial);
        parent.child_by_value.push(by_value);
        self.emit(Op::MakeClosure {
            dst,
            child: child_idx,
        });
        Ok(())
    }

    // assignment
}

/// The type parameter of the function that a written type names bare, `X` in
/// `fn pick<X>(a: X)`.
fn type_param<'s>(sig: &'s syn::Signature, ty: &syn::Type) -> Option<&'s syn::Ident> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    let name = path.path.get_ident()?;
    sig.generics
        .type_params()
        .map(|param| &param.ident)
        .find(|ident| *ident == name)
}
