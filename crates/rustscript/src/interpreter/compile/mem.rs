//! `mem::take`, `mem::replace` and `mem::swap` with a `&mut T` binding as an argument, like
//! `take(word)` in a fn that takes `word: &mut String`. The value moves out through the
//! reference and the new one is stored through it, the way `*word = new` does.

use anyhow::Result;
use syn::Expr;

use super::super::bytecode::{Op, PathId, Reg};
use super::infer::Ty;
use super::place::Place;
use super::{Compiler, idx16};

/// A name or a field that holds a `&mut T`, not a `&mut place` written at the call.
fn is_ref_binding(e: &Expr) -> bool {
    matches!(e, Expr::Path(_) | Expr::Field(_))
}

/// The arguments that name the places, both of `swap` and the first of `take` and `replace`.
fn place_args(kind: PathId, c: &syn::ExprCall) -> impl Iterator<Item = &Expr> {
    let n = if kind == PathId::MemSwap { 2 } else { 1 };
    c.args.iter().take(n)
}

/// Whether a place argument is a `&mut T` binding, which `compile_mem_intrinsic` can't move.
pub(super) fn takes_ref_binding(kind: PathId, c: &syn::ExprCall) -> bool {
    place_args(kind, c).any(is_ref_binding)
}

enum Slot<'a> {
    /// `&mut place` written at the call
    Place(Place),
    /// a `&mut T` binding
    Ref(&'a Expr),
}

fn deref_of(e: &Expr) -> Expr {
    Expr::Unary(syn::ExprUnary {
        attrs: Vec::new(),
        op: syn::UnOp::Deref(syn::token::Star::default()),
        expr: Box::new(e.clone()),
    })
}

impl Compiler<'_> {
    /// False hands the call back, like `compile_mem_intrinsic`.
    pub(super) fn compile_mem_through_ref(
        &mut self,
        dst: Reg,
        kind: PathId,
        c: &syn::ExprCall,
    ) -> Result<bool> {
        let mut slots = Vec::new();
        for arg in place_args(kind, c) {
            match self.mem_slot(arg)? {
                Some(slot) => slots.push(slot),
                None => return Ok(false),
            }
        }
        match (kind, slots.as_slice()) {
            (PathId::MemSwap, [a, b]) if c.args.len() == 2 => {
                let va = self.mem_read(a)?;
                let vb = self.mem_read(b)?;
                self.mem_write(a, vb)?;
                self.mem_write(b, va)?;
                self.emit(Op::LoadUnit { dst });
            }
            (PathId::MemTake, [a]) if c.args.len() == 1 => {
                let old = self.mem_read(a)?;
                let new = self.alloc();
                // a user type's default is built from its declaration, the value alone can't say
                // what the empty variant or the field defaults are
                let ty = match (&c.args[0], self.types.of(&c.args[0])) {
                    (Expr::Reference(r), Ty::Unknown) => self.types.of(&r.expr),
                    (_, ty) => ty,
                };
                match self.default_ir_of(&ty) {
                    Some(ir) => {
                        let f = self.cur();
                        f.defaults.push(ir);
                        let ir = idx16(f.defaults.len() - 1);
                        self.emit(Op::BuildDefault { dst: new, ir });
                    }
                    None => self.emit(Op::DefaultOf { dst: new, src: old }),
                }
                self.mem_write(a, new)?;
                self.emit(Op::Move { dst, src: old });
            }
            (PathId::MemReplace, [a]) if c.args.len() == 2 => {
                let new = self.compile_owned_expr(&c.args[1])?;
                let old = self.mem_read(a)?;
                self.mem_write(a, new)?;
                self.emit(Op::Move { dst, src: old });
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn mem_slot<'a>(&mut self, arg: &'a Expr) -> Result<Option<Slot<'a>>> {
        Ok(match arg {
            Expr::Reference(r) if r.mutability.is_some() => {
                self.compile_place(&r.expr)?.map(Slot::Place)
            }
            e if is_ref_binding(e) => Some(Slot::Ref(e)),
            _ => None,
        })
    }

    /// The value in the slot, moved out.
    fn mem_read(&mut self, slot: &Slot) -> Result<Reg> {
        match slot {
            Slot::Place(place) => {
                self.emit_place_take(place);
                let old = self.alloc();
                self.emit(Op::Move {
                    dst: old,
                    src: place.reg,
                });
                Ok(old)
            }
            Slot::Ref(e) => self.compile_expr(&deref_of(e)),
        }
    }

    fn mem_write(&mut self, slot: &Slot, val: Reg) -> Result<()> {
        match slot {
            Slot::Place(place) => {
                self.emit(Op::Move {
                    dst: place.reg,
                    src: val,
                });
                self.emit_place_writeback(place);
                Ok(())
            }
            Slot::Ref(e) => self.store_through(e, val),
        }
    }
}
