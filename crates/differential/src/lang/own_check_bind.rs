//! The ownership rules of the statements that bind through a pattern.

use crate::lang::own_check::Checker;
use crate::lang::stmt::{ChainLink, Exit, Stmt};

impl Checker {
    pub(super) fn binding_form(&mut self, stmt: &Stmt) {
        match stmt {
            // Every link counts as run, so what a later link moves is gone in the `else` too.
            Stmt::IfLet {
                links,
                then_body,
                else_body,
            } => {
                let mark = self.scope.enter_scope();
                for link in links {
                    self.expr(link.expr());
                    if let ChainLink::Let { pat, expr } = link {
                        self.push_matched(pat, expr);
                    }
                }
                let before = self.scope.snapshot();
                self.stmts(then_body);
                self.scope.exit_scope(mark);
                let then_end = self.scope.snapshot();
                self.scope.restore(&before);
                if let Some(else_body) = else_body {
                    self.body(else_body);
                }
                let else_end = self.scope.snapshot();
                self.scope.merge(&before, &[then_end, else_end]);
            }
            Stmt::WhileLet {
                name, pat, body, ..
            } => {
                self.require(self.scope.can_write(name), || {
                    format!("while let over `{name}`")
                });
                let hidden = self.scope.hide(name);
                let before = self.scope.snapshot();
                self.scope.enter_loop();
                let mark = self.scope.enter_scope();
                self.push_pat(pat);
                self.stmts(body);
                self.scope.exit_scope(mark);
                self.scope.leave_loop();
                self.scope.restore(&before);
                if let Some(hidden) = hidden {
                    self.scope.unhide(hidden);
                }
            }
            // the `else` leaves, so what it moves is still here after
            Stmt::LetElse {
                pat,
                expr,
                else_body,
                exit,
            } => {
                // the bindings stay for the scope, so a reference among them keeps what the
                // scrutinee borrowed
                self.kept_expr(expr, &expr.ty());
                let before = self.scope.snapshot();
                let mark = self.scope.enter_scope();
                self.stmts(else_body);
                if let Exit::Return(Some(value)) = exit {
                    self.expr(value);
                }
                self.scope.exit_scope(mark);
                self.scope.restore(&before);
                self.push_pat(pat);
            }
            Stmt::Match {
                scrutinee, arms, ..
            } => {
                self.expr(scrutinee);
                self.branches(arms.len(), |inner, index| {
                    let arm = &arms[index];
                    inner.push_matched(&arm.pat, scrutinee);
                    if let Some(guard) = &arm.guard {
                        inner.expr(guard);
                    }
                    inner.stmts(&arm.body);
                });
            }
            Stmt::LetLoop {
                name,
                ty,
                body,
                exit,
                value,
                fallback,
                ..
            } => {
                // the value outlives the loop scope, where a kept borrow would end
                self.require(!ty.contains_ref(), || {
                    format!("`{name}` takes a reference out of its loop")
                });
                let before = self.scope.snapshot();
                self.scope.enter_loop();
                let mark = self.scope.enter_scope();
                self.expr(fallback);
                self.stmts(body);
                self.expr(exit);
                self.expr(value);
                self.scope.exit_scope(mark);
                self.scope.leave_loop();
                self.scope.restore(&before);
                // never written, so it needs no `mut`
                self.push_local(name, ty);
            }
            _ => unreachable!("binding_form handles the pattern statements only"),
        }
    }
}
