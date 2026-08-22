//! Control flow, `if`, loops, `match`, `return`, `break` and `continue`.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use syn::{Expr, Pat};

use crate::interpreter::bytecode::{Op, PathRef, Reg};

use super::support::{iterable_is_owned, pattern_borrows, pattern_owns};
use super::walks::flatten_and;

use super::{Compiler, LoopCtx, NameLoc, idx16};

impl Compiler<'_> {
    /// The value moves to a fresh register first, so the retag doesn't touch a local's own slot
    /// and the value stays shared past the drops.
    pub(super) fn compile_return(&mut self, r: &syn::ExprReturn) -> Result<()> {
        let mut src = if let Some(e) = &r.expr {
            self.compile_owned_expr(e)?
        } else {
            let u = self.alloc();
            self.emit(Op::LoadUnit { dst: u });
            u
        };
        if let Some(idx) = self.cur().ret_cast {
            let out = self.alloc();
            self.emit(Op::Cast {
                dst: out,
                src,
                ty: idx,
            });
            src = out;
        }
        if self.ctx.has_drop {
            let depth = self.cur().scope_order.len();
            self.emit_scope_drops(depth);
        }
        self.emit(Op::Ret { src });
        Ok(())
    }

    pub(super) fn compile_if(&mut self, dst: Reg, if_expr: &syn::ExprIf) -> Result<()> {
        // `if let` and let chains, earlier bindings are in scope for later terms and the body
        let terms = flatten_and(&if_expr.cond);
        if terms.iter().any(|t| matches!(t, Expr::Let(_))) {
            self.push_scope();
            let mut else_jumps = Vec::new();
            for term in &terms {
                if let Expr::Let(let_expr) = term {
                    let owned = pattern_owns(&let_expr.pat);
                    let scrut = self.compile_scrutinee(&let_expr.expr, owned)?;
                    let matched = self.alloc();
                    let pat = self.pattern_info(&let_expr.pat)?;
                    if !owned {
                        self.exempt_pattern_binds(pat);
                    }
                    self.emit(Op::TestBind {
                        val: scrut,
                        pat,
                        dst: matched,
                    });
                    else_jumps.push(self.here());
                    self.emit(Op::JumpIfFalse {
                        cond: matched,
                        to: 0,
                    });
                } else {
                    let cond = self.compile_expr(term)?;
                    else_jumps.push(self.here());
                    self.emit(Op::JumpIfFalse { cond, to: 0 });
                }
            }
            self.compile_block_inner(&if_expr.then_branch, dst)?;
            self.emit_scope_drops(1);
            self.pop_scope();
            let jmp_end = self.here();
            self.emit(Op::Jump { to: 0 });
            let else_at = self.mark()?;
            for j in else_jumps {
                self.patch_jump(j, else_at);
            }
            match &if_expr.else_branch {
                Some((_, e)) => self.compile_into(dst, e)?,
                None => self.emit(Op::LoadUnit { dst }),
            }
            let end = self.mark()?;
            self.patch_jump(jmp_end, end);
            return Ok(());
        }
        let jmp_else = self.emit_cond_jump(&if_expr.cond)?;
        self.compile_block(&if_expr.then_branch, dst)?;
        let jmp_end = self.here();
        self.emit(Op::Jump { to: 0 });
        let else_at = self.mark()?;
        self.patch_jump(jmp_else, else_at);
        match &if_expr.else_branch {
            Some((_, e)) => self.compile_into(dst, e)?,
            None => self.emit(Op::LoadUnit { dst }),
        }
        let end = self.mark()?;
        self.patch_jump(jmp_end, end);
        Ok(())
    }

    pub(super) fn compile_while(&mut self, dst: Reg, w: &syn::ExprWhile) -> Result<()> {
        let head = self.here();
        if let Expr::Let(let_expr) = &*w.cond {
            let owned = pattern_owns(&let_expr.pat);
            let scrut = self.compile_scrutinee(&let_expr.expr, owned)?;
            let while_let_depth = self.cur().scope_order.len();
            self.push_scope();
            let matched = self.alloc();
            let pat = self.pattern_info(&let_expr.pat)?;
            if !owned {
                self.exempt_pattern_binds(pat);
            }
            self.emit(Op::TestBind {
                val: scrut,
                pat,
                dst: matched,
            });
            let exit = self.here();
            self.emit(Op::JumpIfFalse {
                cond: matched,
                to: 0,
            });
            self.loops.push(LoopCtx {
                breaks: vec![exit],
                continue_to: head,
                result: dst,
                scope_depth: while_let_depth,
            });
            let body = self.alloc();
            self.compile_block_inner(&w.body, body)?;
            self.emit_scope_drops(1);
            self.pop_scope();
            self.emit(Op::Jump {
                to: u32::try_from(head)?,
            });
            let end = self.mark()?;
            let lc = self.loops.pop().unwrap();
            for b in lc.breaks {
                self.patch_jump(b, end);
            }
            self.emit(Op::LoadUnit { dst });
            return Ok(());
        }
        let exit = self.emit_cond_jump(&w.cond)?;
        let scope_depth = self.cur().scope_order.len();
        self.loops.push(LoopCtx {
            breaks: vec![exit],
            continue_to: head,
            result: dst,
            scope_depth,
        });
        let body = self.alloc();
        self.compile_block(&w.body, body)?;
        self.emit(Op::Jump {
            to: u32::try_from(head)?,
        });
        let end = self.mark()?;
        let lc = self.loops.pop().unwrap();
        for b in lc.breaks {
            self.patch_jump(b, end);
        }
        self.emit(Op::LoadUnit { dst });
        Ok(())
    }

    pub(super) fn compile_loop(&mut self, dst: Reg, l: &syn::ExprLoop) -> Result<()> {
        self.emit(Op::LoadUnit { dst });
        let head = self.here();
        let scope_depth = self.cur().scope_order.len();
        self.loops.push(LoopCtx {
            breaks: Vec::new(),
            continue_to: head,
            result: dst,
            scope_depth,
        });
        let body = self.alloc();
        self.compile_block(&l.body, body)?;
        self.emit(Op::Jump {
            to: u32::try_from(head)?,
        });
        let end = self.mark()?;
        let lc = self.loops.pop().unwrap();
        for b in lc.breaks {
            self.patch_jump(b, end);
        }
        Ok(())
    }

    /// A `&[T]` or `&Vec<T>` parameter, and a `let r = &v` local, both forward the
    /// caller's storage rather than owning it. Iterating one has to borrow, because an
    /// owning iterator drains the elements out from under the caller.
    fn iterates_borrowed(&mut self, expr: &Expr) -> bool {
        let mut inner = expr;
        loop {
            match inner {
                Expr::Paren(p) => inner = &p.expr,
                Expr::Group(g) => inner = &g.expr,
                _ => break,
            }
        }
        let Expr::Path(path) = inner else {
            return false;
        };
        let Some(ident) = path.path.get_ident() else {
            return false;
        };
        match self.resolve(&ident.to_string()) {
            NameLoc::Local(reg) => {
                let state = self.cur();
                state.borrow_params.contains(&reg) || state.ref_locals.contains(&reg)
            }
            _ => false,
        }
    }

    pub(super) fn compile_for(&mut self, dst: Reg, f: &syn::ExprForLoop) -> Result<()> {
        let borrowed = self.iterates_borrowed(&f.expr);
        // `for x in &mut place` lowers to `place.iter_mut()`, so `*x` writes land in the elements
        let src = match &*f.expr {
            Expr::Reference(r) if r.mutability.is_some() => {
                let place = self.compile_mut_receiver(&r.expr)?;
                let name = self.add_name("iter_mut".to_string());
                let out = self.alloc();
                self.emit(Op::Method {
                    dst: out,
                    recv: place.reg,
                    name,
                    base: place.reg,
                    argc: 0,
                });
                out
            }
            e if iterable_is_owned(e) && !borrowed => self.compile_owned_expr(e)?,
            e => self.compile_expr(e)?,
        };
        let owned = iterable_is_owned(&f.expr) && !borrowed;
        let iter = self.alloc();
        self.emit(Op::IterInit {
            dst: iter,
            src,
            owned,
        });
        // an owning iterator drops what a `break` leaves behind, at loop end like real Rust, and
        // at scope end for a `return` out of the loop
        let drops_iter = owned && self.ctx.has_drop;
        if drops_iter {
            self.cur().scope_order.last_mut().unwrap().push(iter);
        }
        let idx = self.alloc();
        self.emit(Op::LoadInt { dst: idx, v: 0 });
        let val = self.alloc();
        let head = self.here();
        let next = self.here();
        self.emit(Op::ForNext {
            iter,
            idx,
            val,
            to: 0,
        });
        let scope_depth = self.cur().scope_order.len();
        self.push_scope();
        let before = self.cur().scope_order.last().map_or(0, Vec::len);
        self.bind_pattern_irrefutable(&f.pat, val)?;
        if !owned {
            let bound: Vec<Reg> = self
                .cur()
                .scope_order
                .last()
                .map_or(Vec::new(), |regs| regs[before..].to_vec());
            self.cur().drop_exempt.extend(bound);
        }
        self.loops.push(LoopCtx {
            breaks: vec![next],
            continue_to: head,
            result: dst,
            scope_depth,
        });
        let body = self.alloc();
        self.compile_block_inner(&f.body, body)?;
        self.emit_scope_drops(1);
        self.pop_scope();
        self.emit(Op::Jump {
            to: u32::try_from(head)?,
        });
        let end = self.mark()?;
        let lc = self.loops.pop().unwrap();
        for b in lc.breaks {
            self.patch_jump(b, end);
        }
        if drops_iter {
            let f = self.cur();
            f.drop_lists.push(Arc::from(vec![iter]));
            let list = idx16(f.drop_lists.len() - 1);
            self.emit(Op::DropScope { list });
        }
        self.emit(Op::LoadUnit { dst });
        Ok(())
    }

    pub(super) fn compile_break(&mut self, b: &syn::ExprBreak) -> Result<()> {
        let result = self.loops.last().map(|l| l.result);
        if let Some(result) = result {
            if let Some(e) = &b.expr {
                self.compile_into(result, e)?;
            }
        } else {
            bail!("break outside a loop");
        }
        self.emit_loop_exit_drops();
        let jmp = self.here();
        self.emit(Op::Jump { to: 0 });
        self.loops.last_mut().unwrap().breaks.push(jmp);
        Ok(())
    }

    pub(super) fn compile_continue(&mut self) -> Result<()> {
        let to = self
            .loops
            .last()
            .map(|l| l.continue_to)
            .ok_or_else(|| anyhow!("continue outside a loop"))?;
        self.emit_loop_exit_drops();
        self.emit(Op::Jump {
            to: u32::try_from(to)?,
        });
        Ok(())
    }

    /// The scopes a `break` or `continue` leaves drop first.
    pub(super) fn emit_loop_exit_drops(&mut self) {
        if !self.ctx.has_drop {
            return;
        }
        let Some(entry) = self.loops.last().map(|l| l.scope_depth) else {
            return;
        };
        let depth = self.cur().scope_order.len().saturating_sub(entry);
        self.emit_scope_drops(depth);
    }

    pub(super) fn compile_match(&mut self, dst: Reg, m: &syn::ExprMatch) -> Result<()> {
        fn arm_pattern(pat: &Pat) -> &Pat {
            match pat {
                Pat::Guard(g) => &g.pat,
                p => p,
            }
        }
        let owned = m.arms.iter().any(|arm| pattern_owns(arm_pattern(&arm.pat)))
            && !m
                .arms
                .iter()
                .any(|arm| pattern_borrows(arm_pattern(&arm.pat)));
        let scrut = self.compile_scrutinee(&m.expr, owned)?;
        let mut end_jumps = Vec::new();
        for arm in &m.arms {
            self.push_scope();
            let matched = self.alloc();
            // syn 3 parses `pat if cond` as `Pat::Guard`
            let (arm_pat, arm_guard) = match &arm.pat {
                Pat::Guard(g) => (&*g.pat, Some(&*g.guard)),
                p => (p, None),
            };
            let pat = self.pattern_info(arm_pat)?;
            if !owned {
                self.exempt_pattern_binds(pat);
            }
            self.emit(Op::TestBind {
                val: scrut,
                pat,
                dst: matched,
            });
            let skip = self.here();
            self.emit(Op::JumpIfFalse {
                cond: matched,
                to: 0,
            });
            let mut guard_skip = None;
            if let Some(guard) = arm_guard {
                let g = self.compile_expr(guard)?;
                let gs = self.here();
                self.emit(Op::JumpIfFalse { cond: g, to: 0 });
                guard_skip = Some(gs);
            }
            self.compile_owned_into(dst, &arm.body)?;
            self.emit_scope_drops(1);
            let je = self.here();
            self.emit(Op::Jump { to: 0 });
            end_jumps.push(je);
            self.pop_scope();
            let next = self.mark()?;
            self.patch_jump(skip, next);
            if let Some(gs) = guard_skip {
                self.patch_jump(gs, next);
            }
        }
        // no arm matched
        let p = self.add_path(PathRef::new(vec!["::unreachable_match".to_string()], None));
        self.emit(Op::CallPath {
            dst,
            path: p,
            base: dst,
            argc: 0,
        });
        let end = self.mark()?;
        for j in end_jumps {
            self.patch_jump(j, end);
        }
        Ok(())
    }

    // calls
}
