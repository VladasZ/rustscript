//! Mutation statements, collection operations, accumulation loops and mutating calls.

use rand::RngExt;

use crate::lang::expr::Expr;
use crate::lang::stmt::{MutOp, Stmt};
use crate::lang::synth::{BindKind, Generator};
use crate::lang::ty::Ty;

impl Generator<'_> {
    /// `name` is hidden because an `entry()` chain holds the map while its arguments evaluate.
    pub(super) fn op_without(
        &mut self,
        name: &str,
        build: impl FnOnce(&mut Self) -> MutOp,
    ) -> MutOp {
        let index = self.scope.iter().position(|binding| binding.name == name);
        let removed = index.map(|found| self.scope.remove(found));
        let op = build(self);
        if let Some(binding) = removed {
            self.scope.push(binding);
        }
        op
    }

    pub(super) fn collection_mutation(&mut self) -> Option<Stmt> {
        let (name, ty) = self.pick_mutable()?;
        let op = match &ty {
            Ty::Vec(elem) => {
                let elem = (**elem).clone();
                match self.rng.random_range(0..11) {
                    0 => MutOp::VecPush(self.expr(&elem, 1)),
                    1 if elem.is_ord() => MutOp::VecSort,
                    2 => match &elem {
                        Ty::Vec(inner) => MutOp::VecRowPush {
                            index: self.rng.random_range(0..=2),
                            value: self.expr(inner, 1),
                        },
                        _ => MutOp::VecDedup,
                    },
                    3 => MutOp::VecSetIndex {
                        index: self.rng.random_range(0..=6),
                        value: self.expr(&elem, 1),
                    },
                    4 => MutOp::VecReverse,
                    5 => MutOp::VecPop,
                    6 => MutOp::VecTruncate(self.rng.random_range(0..=3)),
                    7 => MutOp::VecSwap(self.rng.random_range(0..=4), self.rng.random_range(0..=4)),
                    8 => {
                        let bind = self.fresh("diff_r");
                        let locals = [(bind.clone(), elem.clone())];
                        // `retain` holds the vec, so the predicate must not read it
                        let pred = self.without_binding(&name, |inner| {
                            inner.closure_body(|inner| {
                                inner.with_locals(&locals, |inner| inner.expr(&Ty::Bool, 1))
                            })
                        });
                        MutOp::VecRetain { bind, pred }
                    }
                    9 => MutOp::VecClear,
                    _ => MutOp::VecExtend(self.expr(&Ty::vec_of(elem), 1)),
                }
            }
            Ty::Str => match self.rng.random_range(0..3) {
                0 => MutOp::StrPush(self.expr(&Ty::Char, 1)),
                1 => MutOp::StrClear,
                _ => MutOp::StrPushStr(self.expr(&Ty::Str, 1)),
            },
            Ty::Opt(elem) => {
                if self.chance(0.5) {
                    MutOp::OptTake
                } else {
                    MutOp::OptReplace(self.expr(elem, 1))
                }
            }
            Ty::Map(key, value) => {
                let key_ty = (**key).clone();
                let val_ty = (**value).clone();
                match (&val_ty, self.rng.random_range(0..4)) {
                    (Ty::Int(_), 0) => self.op_without(&name, |inner| MutOp::MapEntryAdd {
                        key: inner.expr(&key_ty, 1),
                        default: inner.expr(&val_ty, 1),
                        add: inner.expr(&val_ty, 1),
                    }),
                    (Ty::Vec(elem), 0) => {
                        let elem = (**elem).clone();
                        self.op_without(&name, |inner| MutOp::MapEntryPush {
                            key: inner.expr(&key_ty, 1),
                            value: inner.expr(&elem, 1),
                        })
                    }
                    (_, 1) => MutOp::MapRemove {
                        key: self.expr(&key_ty, 1),
                    },
                    _ => MutOp::MapInsert {
                        key: self.expr(&key_ty, 1),
                        value: self.expr(&val_ty, 1),
                    },
                }
            }
            Ty::Set(elem) => {
                let elem = (**elem).clone();
                if self.chance(0.7) {
                    MutOp::SetInsert(self.expr(&elem, 1))
                } else {
                    MutOp::SetRemove(self.expr(&elem, 1))
                }
            }
            _ => return None,
        };
        Some(Stmt::Mutate { name, op })
    }

    /// `for item in vec { accumulate into collection }`. The source is a vec, so order is defined.
    pub(super) fn accumulation_loop(&mut self) -> Option<Stmt> {
        let (target, ty) = self.pick_collection()?;
        let var = self.fresh("diff_item");
        let (source_elem, op) = match &ty {
            Ty::Vec(elem) => {
                let elem = (**elem).clone();
                (
                    elem.clone(),
                    MutOp::VecPush(Expr::Var {
                        name: var.clone(),
                        ty: elem,
                    }),
                )
            }
            Ty::Set(elem) => {
                let elem = (**elem).clone();
                (
                    elem.clone(),
                    MutOp::SetInsert(Expr::Var {
                        name: var.clone(),
                        ty: elem,
                    }),
                )
            }
            Ty::Map(key, value) => {
                let key_ty = (**key).clone();
                let val_ty = (**value).clone();
                let key_expr = Expr::Var {
                    name: var.clone(),
                    ty: key_ty.clone(),
                };
                let op = match &val_ty {
                    Ty::Int(_) => self.op_without(&target, |inner| MutOp::MapEntryAdd {
                        key: key_expr,
                        default: inner.expr(&val_ty, 1),
                        add: inner.expr(&val_ty, 1),
                    }),
                    Ty::Vec(elem) => {
                        let elem = (**elem).clone();
                        self.op_without(&target, |inner| MutOp::MapEntryPush {
                            key: key_expr,
                            value: inner.expr(&elem, 1),
                        })
                    }
                    _ => MutOp::MapInsert {
                        key: key_expr,
                        value: self.expr(&val_ty, 1),
                    },
                };
                (key_ty, op)
            }
            _ => return None,
        };
        let source = self.expr(&Ty::vec_of(source_elem), 2);
        Some(Stmt::ForAccum {
            var,
            source,
            target,
            op,
        })
    }

    /// `for r in vec.iter_mut() { *r = expr(r) }` on a vec binding.
    pub(super) fn for_mut_stmt(&mut self) -> Option<Stmt> {
        let vecs: Vec<(String, Ty)> = self
            .scope
            .iter()
            .filter(|binding| matches!(binding.kind, BindKind::Local))
            .filter_map(|binding| match &binding.ty {
                Ty::Vec(elem) => Some((binding.name.clone(), (**elem).clone())),
                _ => None,
            })
            .collect();
        if vecs.is_empty() {
            return None;
        }
        let (name, elem) = self.pick(&vecs).clone();
        let var = self.fresh("diff_e");
        // the vec is borrowed for the loop, so its name is hidden
        let index = self.scope.iter().position(|binding| binding.name == name);
        let removed = index.map(|found| self.scope.remove(found));
        let locals = [(var.clone(), elem.clone())];
        let expr = self.with_locals(&locals, |inner| inner.expr(&elem, 2));
        if let Some(binding) = removed {
            self.scope.push(binding);
        }
        Some(Stmt::ForMut {
            name,
            var,
            elem,
            expr,
        })
    }

    /// `helper(&mut binding, args)`
    pub(super) fn call_mut_stmt(&mut self) -> Option<Stmt> {
        let (name, ty) = self.pick_local()?;
        let (fn_name, params) = self.writer_fn(&ty);
        // the target is borrowed for the call, so the arguments can't read it
        let index = self.scope.iter().position(|binding| binding.name == name);
        let removed = index.map(|found| self.scope.remove(found));
        let args = params.iter().map(|param| self.expr(param, 1)).collect();
        if let Some(binding) = removed {
            self.scope.push(binding);
        }
        Some(Stmt::CallMut {
            name,
            fn_name,
            args,
        })
    }

    pub(super) fn pick_mutable(&mut self) -> Option<(String, Ty)> {
        let matching: Vec<(String, Ty)> = self
            .scope
            .iter()
            .filter(|binding| {
                matches!(binding.kind, BindKind::Local)
                    && matches!(
                        binding.ty,
                        Ty::Vec(_) | Ty::Map(..) | Ty::Set(_) | Ty::Str | Ty::Opt(_)
                    )
            })
            .map(|binding| (binding.name.clone(), binding.ty.clone()))
            .collect();
        if matching.is_empty() {
            return None;
        }
        Some(self.pick(&matching).clone())
    }

    pub(super) fn pick_collection(&mut self) -> Option<(String, Ty)> {
        let matching: Vec<(String, Ty)> = self
            .scope
            .iter()
            .filter(|binding| {
                matches!(binding.kind, BindKind::Local)
                    && matches!(binding.ty, Ty::Vec(_) | Ty::Map(..) | Ty::Set(_))
            })
            .map(|binding| (binding.name.clone(), binding.ty.clone()))
            .collect();
        if matching.is_empty() {
            return None;
        }
        Some(self.pick(&matching).clone())
    }
}
