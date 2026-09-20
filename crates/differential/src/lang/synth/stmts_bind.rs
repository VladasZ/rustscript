//! Generation of the statements that bind through a pattern.

use rand::RngExt;

use crate::lang::expr::{Expr, unbare_deep};
use crate::lang::pat::Pat;
use crate::lang::stmt::{ChainLink, Exit, Stmt, StmtArm};
use crate::lang::synth::{Generator, MAX_EXPR_DEPTH};
use crate::lang::ty::Ty;

impl Generator<'_> {
    /// The bindings of `pat` join the current scope and stay until it ends.
    pub(super) fn push_pat(&mut self, pat: &Pat) {
        let mut binds = Vec::new();
        pat.bindings(&mut binds);
        let borrowed = pat.borrowed();
        for (name, ty) in binds {
            if borrowed.contains(&name) {
                self.scope.push_borrowed(name, ty);
            } else {
                self.push_local(name, ty);
            }
        }
    }

    /// A scrutinee with a refutable pattern over it. A binding forces real literal suffixes,
    /// see `match_expr`.
    fn refutable_pair(&mut self, depth: usize) -> Option<(Pat, Expr)> {
        let ty = self.refutable_ty();
        let pat = self.refutable_pat(&ty)?;
        let expr = unbare_deep(self.expr(&ty, depth));
        Some((pat, expr))
    }

    /// `if let`, now and then with more links and an `else`. The later links run only when
    /// the ones before them matched, so they move nothing.
    pub(super) fn if_let_stmt(&mut self) -> Option<Stmt> {
        let (pat, expr) = self.refutable_pair(MAX_EXPR_DEPTH - 1)?;
        let mut links = vec![ChainLink::Let { pat, expr }];
        self.begin_branches();
        let then_body = self.branch(|inner| {
            if let ChainLink::Let { pat, .. } = &links[0] {
                inner.push_pat(&pat.clone());
            }
            let extra = if inner.chance(0.4) {
                inner.rng.random_range(1..=2)
            } else {
                0
            };
            for _ in 0..extra {
                let link = inner.borrowing(|inner| {
                    if inner.chance(0.5)
                        && let Some((pat, expr)) = inner.refutable_pair(1)
                    {
                        return ChainLink::Let { pat, expr };
                    }
                    ChainLink::Cond(inner.expr(&Ty::Bool, 2))
                });
                if let ChainLink::Let { pat, .. } = &link {
                    inner.push_pat(pat);
                }
                links.push(link);
            }
            inner.nested_body()
        });
        let else_body = self.chance(0.5).then(|| self.branch(Self::nested_body));
        if else_body.is_none() {
            // the path where no link matched
            self.branch(|_| ());
        }
        self.end_branches();
        Some(Stmt::IfLet {
            links,
            then_body,
            else_body,
        })
    }

    /// `while let Some(x) = v.pop()`. The vec is hidden from the body, so it only shrinks.
    pub(super) fn while_let_stmt(&mut self) -> Option<Stmt> {
        let vecs: Vec<(String, Ty)> = self
            .live_locals()
            .into_iter()
            .filter(|(name, ty)| matches!(ty, Ty::Vec(_)) && self.scope.can_write(name))
            .collect();
        if vecs.is_empty() {
            return None;
        }
        let (name, ty) = self.pick(&vecs).clone();
        let elem = ty.elem().cloned()?;
        let payload = self.sub_pat(&elem, 1, false);
        let pat = Pat::Some(Box::new(payload));
        let (body, label) =
            self.without_binding(&name, |inner| inner.loop_body_with(Some(&pat.clone())));
        Some(Stmt::WhileLet {
            name,
            pat,
            body,
            label,
        })
    }

    /// `let pat = expr else { .. };`. The `else` leaves through the nearest loop or the
    /// function, in `main` it ends the program.
    pub(super) fn let_else_stmt(&mut self) -> Option<Stmt> {
        let ty = self.refutable_ty();
        // a `ref` binding would borrow the temporary the scrutinee is
        let pat = self.refutable_value_pat(&ty)?;
        let expr = unbare_deep(self.expr(&ty, MAX_EXPR_DEPTH - 1));
        // the `else` runs on the leaving path only, so what it moves is still here after
        let before = self.scope.snapshot();
        let (else_body, exit) = self.scoped(|inner| {
            let body = vec![inner.statement(Self::observation)];
            let exit = if inner.in_loop {
                let label = if !inner.loop_labels.is_empty() && inner.chance(0.4) {
                    let labels = inner.loop_labels.clone();
                    Some(inner.pick(&labels).clone())
                } else {
                    None
                };
                if inner.chance(0.5) {
                    Exit::Break(label)
                } else {
                    Exit::Continue(label)
                }
            } else if let Some(ret) = inner.fn_ret.clone() {
                Exit::Return(Some(inner.expr(&ret, 2)))
            } else {
                Exit::Return(None)
            };
            (body, exit)
        });
        self.scope.restore(&before);
        self.push_pat(&pat);
        Some(Stmt::LetElse {
            pat,
            expr,
            else_body,
            exit,
        })
    }

    /// A `match` whose arms are statement lists, so an arm may push, assign, print, `break`,
    /// `continue` or `return`.
    pub(super) fn match_stmt(&mut self) -> Option<Stmt> {
        let scrutinee_ty = self.scrutinee_ty()?;
        let scrutinee = self.expr(&scrutinee_ty, MAX_EXPR_DEPTH - 1);
        let by_ref = matches!(scrutinee_ty, Ty::Vec(_));
        self.begin_branches();
        let arms = self.arms_for(&scrutinee_ty, &mut |inner, pat, guard| {
            inner.branch(|inner| {
                inner.with_pat(&pat.clone(), |inner| {
                    let guard = guard.then(|| inner.borrowing(|inner| inner.expr(&Ty::Bool, 2)));
                    let body = inner.nested_body();
                    StmtArm { pat, guard, body }
                })
            })
        });
        self.end_branches();
        let arms = arms?;
        let binds = arms.iter().any(|arm| {
            let mut binds = Vec::new();
            arm.pat.bindings(&mut binds);
            !binds.is_empty()
        });
        let scrutinee = if binds {
            unbare_deep(scrutinee)
        } else {
            scrutinee
        };
        Some(Stmt::Match {
            scrutinee,
            by_ref,
            arms,
        })
    }

    /// `let x = loop { .. break value; };`. A bare `break` would not type check in a loop
    /// that yields a value, so the body sees no loop to leave.
    pub(super) fn let_loop_stmt(&mut self) -> Stmt {
        let ty = self.any_ty();
        let name = self.fresh("v");
        let counter = self.fresh("diff_i");
        let limit = self.rng.random_range(0..=3);
        let saved_loop = std::mem::replace(&mut self.in_loop, false);
        let saved_labels = std::mem::take(&mut self.loop_labels);
        let (fallback, body, exit, value) = self.looping(|inner| {
            // the parts stay in the statement around the loop, so a reference one takes holds
            // its binding for the whole loop
            let fallback = inner.borrowing(|inner| inner.typed_only(|i| i.expr(&ty, 2)));
            let body = inner.nested_body();
            let exit = inner.borrowing(|inner| inner.expr(&Ty::Bool, 2));
            let value = inner.borrowing(|inner| inner.typed_only(|i| i.expr(&ty, 2)));
            (fallback, body, exit, value)
        });
        self.in_loop = saved_loop;
        self.loop_labels = saved_labels;
        self.push_local(name.clone(), ty.clone());
        Stmt::LetLoop {
            name,
            ty,
            counter,
            limit,
            body,
            exit,
            value,
            fallback,
        }
    }
}
