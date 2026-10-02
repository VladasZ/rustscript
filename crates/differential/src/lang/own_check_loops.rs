//! The ownership rules of the `for` loops over a collection.

use crate::lang::expr::Expr;
use crate::lang::own_check::Checker;
use crate::lang::stmt::{ElemWrite, ForForm, MutPlace, Stmt};
use crate::lang::ty::Ty;

impl Checker {
    pub(super) fn loop_form(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::ForAccum {
                var,
                source,
                target,
                op,
            } => {
                self.require(self.scope.can_write(target), || {
                    format!("accumulate into `{target}`")
                });
                self.scope.freeze(target);
                self.expr(source);
                let elem = match source.ty() {
                    Ty::Vec(elem) => *elem,
                    other => other,
                };
                self.loop_with(&[(var.clone(), elem)], &[], |inner| {
                    inner.mut_op(target, op);
                });
                self.scope.unfreeze();
            }
            Stmt::ForMut {
                name,
                var,
                elem,
                write,
            } => {
                self.require(self.scope.can_write(name), || {
                    format!("iter_mut over `{name}`")
                });
                let hidden = self.scope.hide(name);
                self.loop_with(&[(var.clone(), elem.clone())], &[], |inner| {
                    inner.elem_write(write);
                });
                if let Some(hidden) = hidden {
                    self.scope.unhide(hidden);
                }
            }
            Stmt::ForEach {
                var,
                elem,
                source,
                form,
                body,
                ..
            } => {
                let held = self.shared_source(source);
                let mut owned = Vec::new();
                if let ForForm::Enumerate { index } = form {
                    owned.push((index.clone(), Ty::USIZE));
                }
                let borrowed = [(format!("(*{var})"), elem.clone())];
                self.loop_with(&owned, &borrowed, |inner| inner.stmts(body));
                if held {
                    self.scope.release_shared();
                }
            }
            Stmt::ForUnordered {
                name,
                item_ty,
                binds,
                source,
                item,
            } => {
                let held = self.shared_source(source);
                let borrowed: Vec<(String, Ty)> = binds
                    .iter()
                    .map(|(bind, ty)| (format!("(*{bind})"), ty.clone()))
                    .collect();
                self.loop_with(&[], &borrowed, |inner| inner.expr(item));
                if held {
                    self.scope.release_shared();
                }
                self.push_let(name, &Ty::vec_of(item_ty.clone()));
            }
            _ => unreachable!("loop_form handles the collection loops only"),
        }
    }

    /// A `&mut` into a binding. The binding is borrowed for the whole statement, so nothing
    /// in it reads the name.
    pub(super) fn ref_mut(
        &mut self,
        name: &str,
        place: &MutPlace,
        var: &str,
        elem: &Ty,
        write: &ElemWrite,
    ) {
        self.require(self.scope.can_write(name), || format!("&mut into `{name}`"));
        let hidden = self.scope.hide(name);
        match place {
            MutPlace::MapGetMut(key) => self.expr(key),
            MutPlace::Entry { key, default } => {
                self.expr(key);
                self.scope.enter_closure();
                self.expr(default);
                self.scope.leave_closure();
            }
            _ => {}
        }
        let item = [(var.to_string(), elem.clone())];
        if matches!(place, MutPlace::ValuesMut) {
            self.loop_with(&item, &[], |inner| inner.elem_write(write));
        } else if place.is_conditional() {
            self.branches(1, |inner, _| {
                inner.push_local(var, elem);
                inner.elem_write(write);
            });
        } else {
            let mark = self.scope.enter_scope();
            self.push_local(var, elem);
            self.elem_write(write);
            self.scope.exit_scope(mark);
        }
        if let Some(hidden) = hidden {
            self.scope.unhide(hidden);
        }
    }

    /// A write through the `&mut` a statement holds as `diff_ref`.
    fn elem_write(&mut self, write: &ElemWrite) {
        match write {
            ElemWrite::Method(op) => self.mut_op("diff_ref", op),
            other => {
                for expr in other.exprs() {
                    self.expr(expr);
                }
            }
        }
    }

    /// The source of a loop that borrows it. A binding is read in place and held for the loop,
    /// and the answer says so. Anything else is a temporary.
    fn shared_source(&mut self, source: &Expr) -> bool {
        match source {
            Expr::Var { name, .. } => {
                self.require(self.scope.can_read(name), || format!("loop over `{name}`"));
                self.scope.hold_shared(name);
                true
            }
            other => {
                self.expr(other);
                false
            }
        }
    }

    /// A loop body with its item bindings in scope. A `borrowed` one stands behind a
    /// reference. Nothing the body revives counts after it, the loop may run zero times.
    fn loop_with(
        &mut self,
        owned: &[(String, Ty)],
        borrowed: &[(String, Ty)],
        build: impl FnOnce(&mut Self),
    ) {
        let before = self.scope.snapshot();
        self.scope.enter_loop();
        let mark = self.scope.enter_scope();
        for (name, ty) in owned {
            self.push_local(name, ty);
        }
        for (name, ty) in borrowed {
            self.scope.push_borrowed(name.clone(), ty.clone());
        }
        build(self);
        self.scope.exit_scope(mark);
        self.scope.leave_loop();
        self.scope.restore(&before);
    }
}
