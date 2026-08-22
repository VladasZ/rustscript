//! Every pipe built here passes `is_valid`, asserted not assumed.

use rand::RngExt;

use crate::lang::expr::Expr;
use crate::lang::pipe::{
    Access, Bind, Item, ParamAnn, Pipe, Site, Source, Stage, Term, fallible_pending,
};
use crate::lang::synth::Generator;
use crate::lang::ty::Ty;

impl Generator<'_> {
    /// `site` is where a `collect`, `sum` or `product` states its target.
    pub(super) fn pipe_collect(&mut self, want: &Ty, site: Site, depth: usize) -> Option<Expr> {
        if depth == 0 {
            return None;
        }
        let pipe = match want {
            Ty::Vec(elem) | Ty::Set(elem) => {
                self.pipe_to_scalar_collect(elem, want.clone(), site, depth)?
            }
            Ty::Map(key, value) => self.pipe_to_map(key, value, site, depth),
            Ty::Int(_) | Ty::Float(_) => self.pipe_to_number(want, site, depth)?,
            Ty::Bool => self.pipe_to_bool(depth)?,
            Ty::Opt(inner) => self.pipe_to_opt(inner, depth)?,
            _ => return None,
        };
        assert!(
            pipe.is_valid(),
            "generated a pipe that breaks a generator rule: {}",
            pipe.render()
        );
        Some(Expr::Pipe(Box::new(pipe)))
    }

    fn param_ann(&mut self) -> ParamAnn {
        if self.chance(0.35) {
            ParamAnn::Inferred
        } else {
            ParamAnn::Typed
        }
    }

    fn scalar_source(&mut self, depth: usize) -> (Source, bool) {
        match self.rng.random_range(0..6) {
            0 => {
                let start = self.rng.random_range(-20..=20);
                let count = self.rng.random_range(0..=6);
                (Source::Range { start, count }, true)
            }
            1 => {
                let set = self.set_ty();
                let expr = self.expr(&set, depth - 1);
                (
                    Source::Coll {
                        expr,
                        access: Access::SetInto,
                    },
                    false,
                )
            }
            2 => {
                let map = self.map_ty();
                let expr = self.expr(&map, depth - 1);
                let access = if self.chance(0.5) {
                    Access::MapKeys
                } else {
                    Access::MapValues
                };
                (Source::Coll { expr, access }, false)
            }
            _ => {
                let elem = self.elem_ty();
                let expr = self.expr(&Ty::vec_of(elem), depth - 1);
                (
                    Source::Coll {
                        expr,
                        access: Access::VecInto,
                    },
                    true,
                )
            }
        }
    }

    fn fresh_bind(&mut self) -> String {
        self.fresh("diff_x")
    }

    /// A map body states the item type for every later stage, so a bare literal would leave a
    /// `{float}` no method can be called on.
    fn body_with(&mut self, bind: &str, bind_ty: &Ty, want: &Ty, depth: usize) -> Expr {
        let locals = [(bind.to_string(), bind_ty.clone())];
        self.closure_body(|inner| {
            inner.with_locals(&locals, |inner| {
                inner.typed_only(|inner| inner.expr(want, depth))
            })
        })
    }

    fn pipe_to_scalar_collect(
        &mut self,
        elem: &Ty,
        target: Ty,
        site: Site,
        depth: usize,
    ) -> Option<Pipe> {
        let (source, mut ordered) = self.scalar_source(depth);
        let mut stages = Vec::new();
        let mut item = match source.item() {
            Item::Scalar(ty) => ty,
            Item::Pair(..) => return None,
        };
        if item != *elem || self.chance(0.3) {
            let bind = self.fresh_bind();
            let body = self.body_with(&bind, &item, elem, depth - 1);
            if !sort_before_fallible(&mut stages, &mut ordered, &Item::Scalar(item), &body) {
                return None;
            }
            stages.push(Stage::Map {
                bind: Bind::One(bind),
                body,
                ann: self.param_ann(),
            });
            item = elem.clone();
        }
        if self.chance(0.4) {
            let bind = self.fresh_bind();
            let pred = self.body_with(&bind, &item, &Ty::Bool, depth - 1);
            if !sort_before_fallible(
                &mut stages,
                &mut ordered,
                &Item::Scalar(item.clone()),
                &pred,
            ) {
                return None;
            }
            stages.push(Stage::Filter {
                bind: Bind::One(bind),
                pred,
                ann: self.param_ann(),
            });
        }
        // a vec keeps arrival order, a set forgets it
        if matches!(target, Ty::Vec(_)) {
            if !ordered {
                if !item.is_ord() {
                    return None;
                }
                stages.push(Stage::Sorted);
            }
            if self.chance(0.3) {
                // see the panic reach rule in `pipe`
                let choices = if fallible_pending(&stages) { 3 } else { 4 };
                stages.push(match self.rng.random_range(0..choices) {
                    0 => Stage::Rev,
                    1 => Stage::Take(self.rng.random_range(0..=5)),
                    2 => Stage::StepBy(self.rng.random_range(0..=3)),
                    _ => Stage::Skip(self.rng.random_range(0..=3)),
                });
            }
        }
        if matches!(target, Ty::Set(_)) && !item.is_key() {
            return None;
        }
        Some(Pipe {
            source,
            stages,
            term: Term::Collect { target, site },
        })
    }

    /// 3 roads to a pair, a map source, a scalar source paired with a computed value, or an
    /// enumerated ordered source.
    fn pipe_to_map(&mut self, key: &Ty, value: &Ty, site: Site, depth: usize) -> Pipe {
        let target = Ty::map_of(key.clone(), value.clone());
        let choice = self.rng.random_range(0..3);
        if choice == 0 {
            let expr = self.expr(&target, depth - 1);
            let mut stages = Vec::new();
            if self.chance(0.4) {
                let key_bind = self.fresh_bind();
                let val_bind = self.fresh_bind();
                let locals = [
                    (key_bind.clone(), key.clone()),
                    (val_bind.clone(), value.clone()),
                ];
                let pred = self.closure_body(|inner| {
                    inner.with_locals(&locals, |inner| inner.expr(&Ty::Bool, depth - 1))
                });
                let mut ordered = false;
                let pair = Item::Pair(key.clone(), value.clone());
                // when the pair can't sort, the filter is dropped rather than the whole pipe
                if sort_before_fallible(&mut stages, &mut ordered, &pair, &pred) {
                    stages.push(Stage::Filter {
                        bind: Bind::Pair(key_bind, val_bind),
                        pred,
                        ann: ParamAnn::Typed,
                    });
                }
            }
            return Pipe {
                source: Source::Coll {
                    expr,
                    access: Access::MapPairs,
                },
                stages,
                term: Term::Collect { target, site },
            };
        }
        if choice == 1 && *key == Ty::I64 {
            // enumerate needs order
            let expr = self.expr(&Ty::vec_of(value.clone()), depth - 1);
            return Pipe {
                source: Source::Coll {
                    expr,
                    access: Access::VecInto,
                },
                stages: vec![Stage::Enumerate],
                term: Term::Collect { target, site },
            };
        }
        let expr = self.expr(&Ty::vec_of(key.clone()), depth - 1);
        let bind = self.fresh_bind();
        let body = self.body_with(&bind, key, value, depth - 1);
        Pipe {
            source: Source::Coll {
                expr,
                access: Access::VecInto,
            },
            stages: vec![Stage::PairWith {
                bind: Bind::One(bind),
                body,
            }],
            term: Term::Collect { target, site },
        }
    }

    fn pipe_to_number(&mut self, want: &Ty, site: Site, depth: usize) -> Option<Pipe> {
        let (source, mut ordered) = self.scalar_source(depth);
        let item = match source.item() {
            Item::Scalar(ty) => ty,
            Item::Pair(..) => return None,
        };
        if *want == Ty::USIZE && self.chance(0.4) {
            let bind = self.fresh_bind();
            let pred = self.body_with(&bind, &item, &Ty::Bool, depth - 1);
            let mut stages = Vec::new();
            if !sort_before_fallible(&mut stages, &mut ordered, &Item::Scalar(item), &pred) {
                return None;
            }
            stages.push(Stage::Filter {
                bind: Bind::One(bind),
                pred,
                ann: self.param_ann(),
            });
            return Some(Pipe {
                source,
                stages,
                term: Term::Count,
            });
        }
        let mut stages = Vec::new();
        let mut item = item;
        if item != *want {
            let bind = self.fresh_bind();
            let body = self.body_with(&bind, &item, want, depth - 1);
            if !sort_before_fallible(&mut stages, &mut ordered, &Item::Scalar(item), &body) {
                return None;
            }
            stages.push(Stage::Map {
                bind: Bind::One(bind),
                body,
                ann: self.param_ann(),
            });
            item = want.clone();
        }
        if ordered && self.chance(0.3) {
            let acc = self.fresh_bind();
            let bind = self.fresh_bind();
            // a bare literal init leaves a `{float}` no method can be called on
            let init = self.typed_only(|inner| inner.expr(want, depth - 1));
            let locals = [(acc.clone(), want.clone()), (bind.clone(), item)];
            let body = self.closure_body(|inner| {
                inner.with_locals(&locals, |inner| inner.expr(want, depth - 1))
            });
            return Some(Pipe {
                source,
                stages,
                term: Term::Fold {
                    acc,
                    bind: Bind::One(bind),
                    init,
                    body,
                },
            });
        }
        let product = self.chance(0.3);
        // signed sums, float sums and products all depend on order
        let order_matters =
            product || want.contains_float() || matches!(want, Ty::Int(width) if width.is_signed());
        if !ordered && order_matters {
            if !item.is_ord() {
                return None;
            }
            stages.push(Stage::Sorted);
        }
        let term = if product {
            Term::Product {
                out: want.clone(),
                site,
            }
        } else {
            Term::Sum {
                out: want.clone(),
                site,
            }
        };
        Some(Pipe {
            source,
            stages,
            term,
        })
    }

    fn pipe_to_bool(&mut self, depth: usize) -> Option<Pipe> {
        let (source, mut ordered) = self.scalar_source(depth);
        let item = match source.item() {
            Item::Scalar(ty) => ty,
            Item::Pair(..) => return None,
        };
        let bind = self.fresh_bind();
        let pred = self.body_with(&bind, &item, &Ty::Bool, depth - 1);
        let mut stages = Vec::new();
        if !sort_before_fallible(&mut stages, &mut ordered, &Item::Scalar(item), &pred) {
            return None;
        }
        let term = if self.chance(0.5) {
            Term::Any {
                bind: Bind::One(bind),
                pred,
            }
        } else {
            Term::All {
                bind: Bind::One(bind),
                pred,
            }
        };
        Some(Pipe {
            source,
            stages,
            term,
        })
    }

    fn pipe_to_opt(&mut self, inner: &Ty, depth: usize) -> Option<Pipe> {
        if *inner == Ty::USIZE && self.chance(0.3) {
            let elem = self.elem_ty();
            let expr = self.expr(&Ty::vec_of(elem.clone()), depth - 1);
            let bind = self.fresh_bind();
            let pred = self.body_with(&bind, &elem, &Ty::Bool, depth - 1);
            return Some(Pipe {
                source: Source::Coll {
                    expr,
                    access: Access::VecInto,
                },
                stages: Vec::new(),
                term: Term::Position {
                    bind: Bind::One(bind),
                    pred,
                },
            });
        }
        let (source, mut ordered) = self.scalar_source(depth);
        let item = match source.item() {
            Item::Scalar(ty) => ty,
            Item::Pair(..) => return None,
        };
        let mut stages = Vec::new();
        if item != *inner {
            let bind = self.fresh_bind();
            let body = self.body_with(&bind, &item, inner, depth - 1);
            if !sort_before_fallible(&mut stages, &mut ordered, &Item::Scalar(item), &body) {
                return None;
            }
            stages.push(Stage::Map {
                bind: Bind::One(bind),
                body,
                ann: self.param_ann(),
            });
        }
        let term = match self.rng.random_range(0..4) {
            0 | 1 if ordered => {
                if self.chance(0.5) {
                    Term::Last
                } else {
                    Term::Nth(self.rng.random_range(0..=4))
                }
            }
            _ => {
                if !inner.is_ord() {
                    return None;
                }
                if self.chance(0.5) {
                    Term::Min
                } else {
                    Term::Max
                }
            }
        };
        Some(Pipe {
            source,
            stages,
            term,
        })
    }
}

/// A fallible body on unordered items sorts first. When the items can't sort, report false so the
/// caller gives the pipe up.
fn sort_before_fallible(
    stages: &mut Vec<Stage>,
    ordered: &mut bool,
    item: &Item,
    body: &Expr,
) -> bool {
    if *ordered || !body.has_fallible_op() {
        return true;
    }
    if !item.is_ord() {
        return false;
    }
    stages.push(Stage::Sorted);
    *ordered = true;
    true
}
