//! Closures, spawned blocks and the `&mut` argument writebacks.

use std::sync::Arc;

use anyhow::Result;
use syn::{Expr, Pat};

use crate::interpreter::bytecode::{CapSource, Op, Reg};

use super::place;
use super::{Compiler, FnState, NameLoc, idx16, numeric_annotation};

impl Compiler<'_> {
    /// The callee worked on the arg window copy and the VM hands it back on return. Only for calls
    /// whose window survives, a `CallPath` consumes its args.
    pub(super) fn emit_mut_arg_writebacks<'e>(
        &mut self,
        args: impl Iterator<Item = &'e Expr>,
        base: Reg,
    ) -> Result<()> {
        for (i, arg) in args.enumerate() {
            if let Expr::Reference(r) = arg
                && r.mutability.is_some()
                && let Expr::Path(p) = &*r.expr
                && p.path.segments.len() == 1
                && p.qself.is_none()
            {
                let name = p.path.segments[0].ident.to_string();
                let location = self.resolve_for_write(&name);
                self.emit_name_store(location, base + idx16(i), &name)?;
                continue;
            }
            // the window slot clears after the move, a stale copy would inflate `Rc::strong_count`
            if let Some(reg) = self.borrowed_local(arg) {
                self.emit(Op::Move {
                    dst: reg,
                    src: base + idx16(i),
                });
                self.emit(Op::LoadUnit {
                    dst: base + idx16(i),
                });
            }
        }
        Ok(())
    }

    /// A mutable local lives in a cell and a `&mut` alias points elsewhere, so both stay out.
    pub(super) fn borrowed_local(&mut self, arg: &Expr) -> Option<Reg> {
        let (name, forwarded) = match arg {
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
    /// depth. The writebacks restore the registers.
    pub(super) fn emit_borrow_takes<'e>(&mut self, args: impl Iterator<Item = &'e Expr>) {
        let regs: Vec<Reg> = args.filter_map(|arg| self.borrowed_local(arg)).collect();
        for reg in regs {
            self.emit(Op::LoadUnit { dst: reg });
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
        let ret = self.alloc();
        self.compile_block(block, ret)?;
        self.emit(Op::Ret { src: ret });
        let child = self.frames.pop().unwrap();
        let caps: Vec<CapSource> = child.upvalues.iter().map(|(_, s)| *s).collect();
        let mut chunk = child.into_chunk(self.ctx.file.clone())?;
        chunk.module = idx16(self.ctx.module);
        chunk.moves = moves;
        let parent = self.cur();
        let child_idx = idx16(parent.children.len());
        parent.children.push(Arc::new(chunk));
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
        // a pattern param binds more registers, so every param slot is claimed before any binding
        let regs: Vec<Reg> = params.iter().map(|_| self.alloc()).collect();
        for (p, reg) in params.iter().zip(regs) {
            // a reference param shares the caller's storage, so it never splits
            if let Pat::Type(t) = p
                && matches!(&*t.ty, syn::Type::Reference(_))
            {
                self.cur().borrow_params.insert(reg);
            }
            match p {
                Pat::Ident(id) if id.subpat.is_none() => self.define(&id.ident.to_string(), reg),
                _ => self.bind_pattern_irrefutable(p, reg)?,
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
        }
        if let syn::ReturnType::Type(_, ty) = &c.output
            && numeric_annotation(ty).is_some()
        {
            let idx = self.add_cast(ty);
            self.cur().ret_cast = Some(idx);
        }
        let ret = self.alloc();
        self.compile_into(ret, &c.body)?;
        self.release_guard_temps(0, Some(ret));
        if let Some(idx) = self.cur().ret_cast {
            self.emit(Op::Cast {
                dst: ret,
                src: ret,
                ty: idx,
            });
        }
        self.emit(Op::Ret { src: ret });
        let child = self.frames.pop().unwrap();
        let caps: Vec<CapSource> = child.upvalues.iter().map(|(_, s)| *s).collect();
        let mut chunk = child.into_chunk(self.ctx.file.clone())?;
        chunk.module = idx16(self.ctx.module);
        chunk.moves = c.capture.is_some();
        let chunk = Arc::new(chunk);
        let parent = self.cur();
        let child_idx = idx16(parent.children.len());
        parent.children.push(chunk);
        parent.child_caps.push(caps);
        self.emit(Op::MakeClosure {
            dst,
            child: child_idx,
        });
        Ok(())
    }

    // assignment
}
