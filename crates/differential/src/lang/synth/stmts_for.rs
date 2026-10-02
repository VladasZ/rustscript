//! Generation of the `for` loops that borrow a collection, shared or by `iter_mut`.

use rand::RngExt;

use crate::lang::expr::{BinOp, Expr, ReadMode};
use crate::lang::pat::{BindBy, Pat};
use crate::lang::stmt::{ElemWrite, ForForm, MutOp, Stmt};
use crate::lang::synth::Generator;
use crate::lang::ty::Ty;

impl Generator<'_> {
    /// `for r in vec.iter_mut() { write through r }` on a vec binding.
    pub(super) fn for_mut_stmt(&mut self) -> Option<Stmt> {
        let vecs: Vec<(String, Ty)> = self
            .live_locals()
            .into_iter()
            .filter(|(name, _)| self.scope.can_write(name))
            .filter_map(|(name, ty)| match ty {
                Ty::Vec(elem) => Some((name, *elem)),
                _ => None,
            })
            .collect();
        if vecs.is_empty() {
            return None;
        }
        let (name, elem) = self.pick(&vecs).clone();
        let var = self.fresh("diff_e");
        // the vec is borrowed for the loop, so its name is hidden
        let write = self.without_binding(&name, |inner| {
            inner.looping(|inner| {
                inner.push_local(var.clone(), elem.clone());
                inner.elem_write(&elem)
            })
        });
        Some(Stmt::ForMut {
            name,
            var,
            elem,
            write,
        })
    }

    /// A write through the `&mut` of one element, picked by its type.
    fn elem_write(&mut self, elem: &Ty) -> ElemWrite {
        if self.chance(0.4) {
            return ElemWrite::Assign(self.expr(elem, 2));
        }
        match elem {
            Ty::Int(_) | Ty::Float(_) | Ty::Bool => match self.compound_op(elem) {
                Some(op) => {
                    let rhs = if matches!(op, BinOp::Shl | BinOp::Shr) {
                        Ty::U32
                    } else {
                        elem.clone()
                    };
                    ElemWrite::Compound(op, self.expr(&rhs, 2))
                }
                None => ElemWrite::Assign(self.expr(elem, 2)),
            },
            Ty::Vec(inner) => self.method_write(|me| match me.rng.random_range(0..7) {
                0 if inner.is_ord() => MutOp::VecSort,
                1 => MutOp::VecReverse,
                2 => MutOp::VecPop,
                3 => MutOp::VecClear,
                4 => MutOp::VecTruncate(me.rng.random_range(0..=2)),
                5 => MutOp::VecExtend(me.expr(elem, 1)),
                _ => MutOp::VecPush(me.expr(inner, 1)),
            }),
            Ty::Str => self.method_write(|me| match me.rng.random_range(0..4) {
                0 => MutOp::StrPush(me.expr(&Ty::Char, 1)),
                1 => MutOp::StrClear,
                _ => MutOp::StrPushStr(me.expr(&Ty::Str, 1)),
            }),
            Ty::Opt(inner) => self.method_write(|me| {
                if me.chance(0.5) {
                    MutOp::OptTake
                } else {
                    MutOp::OptReplace(me.expr(inner, 1))
                }
            }),
            _ => ElemWrite::Assign(self.expr(elem, 2)),
        }
    }

    fn method_write(&mut self, build: impl FnOnce(&mut Self) -> MutOp) -> ElemWrite {
        ElemWrite::Method(Box::new(build(self)))
    }

    /// A collection to loop over by shared reference. A live binding is named in place and
    /// comes back as the name to hold, anything else is a temporary.
    fn shared_source(
        &mut self,
        fits: impl Fn(&Ty) -> bool,
        fresh: impl FnOnce(&mut Self) -> Ty,
    ) -> Option<(Expr, Option<String>)> {
        let places: Vec<(String, Ty)> = self
            .live_locals()
            .into_iter()
            .filter(|(_, ty)| fits(ty))
            .collect();
        if !places.is_empty() && self.chance(0.75) {
            let (name, ty) = self.pick(&places).clone();
            let source = Expr::Var {
                name: name.clone(),
                ty,
                mode: ReadMode::Clone,
            };
            return Some((source, Some(name)));
        }
        let ty = fresh(self);
        let source = match self.call(&ty, 2) {
            Some(expr) => expr,
            None => self.literal(&ty),
        };
        // a read of a binding would render as the binding itself, which is the place form
        (!matches!(source, Expr::Var { .. })).then_some((source, None))
    }

    /// `for x in &v`, `for x in v.iter()` or `for (i, x) in v.iter().enumerate()`. The item
    /// stands behind a reference, and a source binding is held for the whole loop.
    pub(super) fn for_each_stmt(&mut self) -> Option<Stmt> {
        let (source, held) = self.shared_source(
            |ty| matches!(ty, Ty::Vec(_)),
            |inner| Ty::vec_of(inner.elem_ty()),
        )?;
        let elem = source.ty().elem().cloned()?;
        let var = self.fresh("diff_e");
        let form = match self.rng.random_range(0..3) {
            0 => ForForm::Ref,
            1 => ForForm::Iter,
            _ => ForForm::Enumerate {
                index: self.fresh("diff_i"),
            },
        };
        // never rendered, it only carries the bindings into the body
        let mut binds = Vec::new();
        if let ForForm::Enumerate { index } = &form {
            binds.push(Pat::Bind {
                name: index.clone(),
                ty: Ty::USIZE,
                by: BindBy::Value,
            });
        }
        binds.push(Pat::Bind {
            name: var.clone(),
            ty: elem.clone(),
            by: BindBy::Ref,
        });
        if let Some(name) = &held {
            self.scope.hold_shared(name);
        }
        let (body, label) = self.loop_body_with(Some(&Pat::Tuple(binds)));
        if held.is_some() {
            self.scope.release_shared();
        }
        Some(Stmt::ForEach {
            var,
            elem,
            source,
            form,
            body,
            label,
        })
    }

    /// `for (k, v) in &map` or `for x in &set`. The order is random per process, so the body
    /// only collects one value per entry and the statement sorts them after the loop.
    pub(super) fn for_unordered_stmt(&mut self) -> Option<Stmt> {
        let (source, _) = self.shared_source(
            |ty| matches!(ty, Ty::Map(..) | Ty::Set(_)),
            |inner| {
                if inner.chance(0.6) {
                    inner.map_ty()
                } else {
                    inner.set_ty()
                }
            },
        )?;
        let binds: Vec<(String, Ty)> = match source.ty() {
            Ty::Map(key, value) => {
                vec![(self.fresh("diff_k"), *key), (self.fresh("diff_v"), *value)]
            }
            Ty::Set(elem) => vec![(self.fresh("diff_e"), *elem)],
            _ => return None,
        };
        let locals: Vec<(String, Ty)> = binds
            .iter()
            .map(|(bind, ty)| (format!("(*{bind})"), ty.clone()))
            .collect();
        let (item_ty, item) = self.unordered_item(&locals);
        let name = self.fresh("v");
        self.push_let(name.clone(), Ty::vec_of(item_ty.clone()));
        Some(Stmt::ForUnordered {
            name,
            item_ty,
            binds,
            source,
            item,
        })
    }

    /// The value one entry adds. It sees the entry alone and can neither panic nor print a
    /// drop, a different entry order would show in both. The entry itself is the fallback.
    fn unordered_item(&mut self, locals: &[(String, Ty)]) -> (Ty, Expr) {
        for _ in 0..4 {
            let ty = self.scalar_ty();
            if !ty.is_ord() {
                continue;
            }
            let item = self.closure_body(|inner| {
                inner.without_scope(|inner| inner.with_borrowed(locals, |inner| inner.expr(&ty, 2)))
            });
            let reads_entry = locals
                .iter()
                .any(|(name, _)| item.uses_any(&[name.clone()].into()));
            if reads_entry && !item.has_fallible_op() && !item.mentions_trace() {
                return (ty, item);
            }
        }
        let reads: Vec<Expr> = locals
            .iter()
            .map(|(name, ty)| Expr::Var {
                name: name.clone(),
                ty: ty.clone(),
                mode: ReadMode::Clone,
            })
            .collect();
        let item = Expr::TupleLit(reads);
        (item.ty(), item)
    }
}
