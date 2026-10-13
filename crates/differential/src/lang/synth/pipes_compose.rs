//! Pipes as a free chain. The solver picks a terminal for the wanted type, then a source, then
//! the stages between them one at a time, each from the ones that fit the chain so far.

use std::mem::take;

use rand::RngExt;

use crate::lang::expr::{BorrowKind, Expr};
use crate::lang::pipe::{
    Access, Bind, Caps, Item, Pipe, Site, Source, Stage, Term, fallible_pending, held_names,
};
use crate::lang::ref_param::RefParam;
use crate::lang::synth::Generator;
use crate::lang::ty::{IntWidth, MAX_TY_DEPTH, Ty};

/// The terminal a composed pipe ends in.
#[derive(Clone, Copy)]
enum End {
    CollectVec,
    CollectString,
    CollectResult,
    ForEach,
    Partition,
    Unzip,
    Find,
    FindMap,
    MaxByKey,
    MinByKey,
    Reduce,
    Rposition,
    Fold,
}

/// A stage that keeps the item type.
#[derive(Clone, Copy)]
enum Keep {
    Filter,
    TakeWhile,
    SkipWhile,
    Take,
    StepBy,
    Peekable,
    Chain,
    Rev,
    Skip,
    Inspect,
    ByRef,
}

/// A chain while it grows, with the item and the capabilities after its last stage.
struct Draft {
    source: Source,
    stages: Vec<Stage>,
    item: Item,
    caps: Caps,
}

impl Draft {
    fn new(source: Source) -> Self {
        let item = source.item();
        let caps = source.caps();
        Self {
            source,
            stages: Vec::new(),
            item,
            caps,
        }
    }

    fn push(&mut self, stage: Stage) {
        self.caps = stage.caps(self.caps);
        self.item = stage.out(&self.item);
        self.stages.push(stage);
    }

    /// See the panic reach rule in `pipe`.
    fn pending(&self) -> bool {
        fallible_pending(&self.source.item(), &self.stages)
    }
}

impl Generator<'_> {
    /// A pipe of type `want` built stage by stage, or nothing when no terminal fits the site.
    pub(super) fn compose_pipe(&mut self, want: &Ty, site: Site, depth: usize) -> Option<Pipe> {
        let end = self.pick_end(want, site)?;
        // the value of a pipe holds no reference, so every borrow in it ends with the statement
        let saved = take(&mut self.refs);
        let pipe = self.compose_to(end, want, site, depth);
        self.refs = saved;
        pipe.filter(Pipe::is_valid)
    }

    fn compose_to(&mut self, end: End, want: &Ty, site: Site, depth: usize) -> Option<Pipe> {
        let need = self.needed_item(end, want);
        let source = self.compose_source(need.as_ref(), depth);
        let mut draft = Draft::new(source);
        if !draft.source.ordered() {
            if !draft.item.is_ord() {
                return None;
            }
            draft.push(Stage::Sorted);
        }
        for _ in 0..self.rng.random_range(0..=2) {
            self.free_stage(&mut draft, depth);
        }
        if let Some(need) = &need
            && (draft.item.ty() != *need || self.chance(0.25))
        {
            self.adapt(&mut draft, need, depth);
        }
        for _ in 0..self.rng.random_range(0..=2) {
            self.keep_stage(&mut draft, depth);
        }
        let term = self.end_term(end, want, site, &mut draft, depth)?;
        Some(Pipe {
            source: draft.source,
            stages: draft.stages,
            term,
        })
    }

    /// A bare site has only its annotation to state the type, so it takes a `collect` alone.
    fn pick_end(&mut self, want: &Ty, site: Site) -> Option<End> {
        let mut ends = Vec::new();
        match want {
            Ty::Vec(_) => ends.extend([End::CollectVec, End::CollectVec, End::CollectVec]),
            Ty::Str => ends.extend([End::CollectString, End::CollectString]),
            Ty::Res(ok, _) if matches!(**ok, Ty::Vec(_)) => {
                ends.extend([End::CollectResult, End::CollectResult]);
            }
            _ => {}
        }
        if site == Site::Bare {
            return (!ends.is_empty()).then(|| *self.pick(&ends));
        }
        match want {
            Ty::Vec(_) => ends.push(End::ForEach),
            Ty::Tuple(parts) if parts.len() == 2 => {
                if let (Ty::Vec(first), Ty::Vec(second)) = (&parts[0], &parts[1]) {
                    ends.extend([End::Unzip, End::Unzip]);
                    if first == second {
                        ends.extend([End::Partition, End::Partition]);
                    }
                }
            }
            Ty::Opt(inner) => {
                ends.extend([
                    End::Find,
                    End::FindMap,
                    End::MaxByKey,
                    End::MinByKey,
                    End::Reduce,
                ]);
                if **inner == Ty::USIZE {
                    ends.extend([End::Rposition, End::Rposition]);
                }
            }
            _ => {}
        }
        ends.push(End::Fold);
        Some(*self.pick(&ends))
    }

    /// The item type the terminal takes, when it takes only one.
    fn needed_item(&mut self, end: End, want: &Ty) -> Option<Ty> {
        match (end, want) {
            (End::CollectVec, Ty::Vec(elem)) => Some((**elem).clone()),
            (End::CollectString, _) => Some(if self.chance(0.5) { Ty::Char } else { Ty::Str }),
            (End::CollectResult, Ty::Res(ok, err)) => {
                let item_ok = ok.elem().cloned().unwrap_or(Ty::I64);
                Some(Ty::res_of(item_ok, (**err).clone()))
            }
            (End::Partition, Ty::Tuple(parts)) => parts[0].elem().cloned(),
            (End::Unzip, Ty::Tuple(parts)) => {
                let half = |part: &Ty| part.elem().cloned().unwrap_or(Ty::I64);
                Some(Ty::Tuple(vec![half(&parts[0]), half(&parts[1])]))
            }
            (End::Find | End::MaxByKey | End::MinByKey | End::Reduce, Ty::Opt(inner)) => {
                Some((**inner).clone())
            }
            _ => None,
        }
    }

    /// A `&str` or a `&[T]` as the receiver of the source call, `s.chars()` for a binding.
    fn source_recv(&mut self, ty: &Ty, depth: usize) -> Expr {
        let expr = self.expr(ty, depth - 1);
        self.respell(expr, BorrowKind::Recv)
    }

    /// Now and then the source already has the item the terminal takes, so the chain needs no
    /// stage that computes it.
    fn compose_source(&mut self, need: Option<&Ty>, depth: usize) -> Source {
        let text = Ty::StrRef;
        if let Some(need) = need
            && self.chance(0.35)
        {
            let access = match need {
                Ty::Char => Access::Chars,
                Ty::Str if self.chance(0.5) => Access::Lines,
                _ if self.chance(0.5) => Access::VecInto,
                _ => Access::SliceIter {
                    copied: need.is_copy() && self.chance(0.5),
                },
            };
            let expr = match access {
                Access::Chars | Access::Lines => self.source_recv(&text, depth),
                Access::VecInto => self.expr(&Ty::vec_of(need.clone()), depth - 1),
                _ => self.source_recv(&Ty::slice_of(need.clone()), depth),
            };
            return Source::Coll { expr, access };
        }
        let size = *self.pick(&[1u8, 1, 2, 2, 2, 3, 3, 0]);
        let access = match self.rng.random_range(0..13) {
            0..=3 => return self.scalar_source(depth).0,
            4..=6 => {
                let elem = self.elem_ty();
                let copied = elem.is_copy() && self.chance(0.5);
                let expr = self.source_recv(&Ty::slice_of(elem), depth);
                return Source::Coll {
                    expr,
                    access: Access::SliceIter { copied },
                };
            }
            7 => Access::Chars,
            8 => Access::Bytes,
            9 => Access::Lines,
            10 => Access::Split(*self.pick(&[',', ' ', 'a', '\n'])),
            11 => Access::Windows(size),
            _ => Access::Chunks(size),
        };
        let recv = match access {
            Access::Windows(_) | Access::Chunks(_) => Ty::slice_of(self.elem_ty()),
            _ => text,
        };
        let expr = self.source_recv(&recv, depth);
        Source::Coll { expr, access }
    }

    /// An expression that runs when the chain is built. The closures of the stages so far hold
    /// what they name, so it takes none of that.
    fn built<T>(&mut self, stages: &[Stage], build: impl FnOnce(&mut Self) -> T) -> T {
        let held = held_names(stages);
        for name in &held {
            self.scope.freeze(name);
        }
        let out = build(self);
        for _ in &held {
            self.scope.unfreeze();
        }
        out
    }

    /// The names of a closure that takes its item by value. A pair item is taken apart in the
    /// parameter more often than not.
    fn value_bind(&mut self, item: &Item) -> (Bind, Vec<(String, Ty)>) {
        match item {
            Item::Pair(key, value) if self.chance(0.6) => {
                let first = self.fresh_bind();
                let second = self.fresh_bind();
                let locals = vec![
                    (first.clone(), key.clone()),
                    (second.clone(), value.clone()),
                ];
                (Bind::Pair(first, second), locals)
            }
            _ => {
                let name = self.fresh_bind();
                let locals = vec![(name.clone(), item.ty())];
                (Bind::One(name), locals)
            }
        }
    }

    /// A closure over the item by value that makes a `want`.
    fn value_closure(&mut self, item: &Item, want: &Ty, depth: usize) -> (Bind, Expr) {
        let (bind, locals) = self.value_bind(item);
        let body = self.closure_body(|inner| {
            inner.with_locals(&locals, |inner| {
                inner.typed_only(|inner| inner.expr(want, depth))
            })
        });
        (bind, body)
    }

    /// A closure that is handed `&item` and makes a `want`.
    fn ref_closure(&mut self, item: &Item, want: &Ty, depth: usize) -> (Bind, RefParam, Expr) {
        let name = self.fresh_bind();
        let (by, body) = self.body_by(&name, &item.ty(), want, depth);
        (Bind::One(name), by, body)
    }

    fn push_map(&mut self, draft: &mut Draft, to: &Ty, depth: usize) {
        let (bind, body) = self.value_closure(&draft.item, to, depth - 1);
        let ann = self.param_ann();
        draft.push(Stage::Map { bind, body, ann });
    }

    fn push_filter_map(&mut self, draft: &mut Draft, to: &Ty, depth: usize) {
        let made = Ty::opt_of(to.clone());
        let (bind, body) = self.value_closure(&draft.item, &made, depth - 1);
        let ann = self.param_ann();
        draft.push(Stage::FilterMap { bind, body, ann });
    }

    fn push_flat_map(&mut self, draft: &mut Draft, to: &Ty, depth: usize) {
        let made = Ty::vec_of(to.clone());
        let (bind, body) = self.value_closure(&draft.item, &made, depth - 1);
        let ann = self.param_ann();
        draft.push(Stage::FlatMap { bind, body, ann });
    }

    fn push_zip(&mut self, draft: &mut Draft, with: &Ty, depth: usize) {
        let other_ty = Ty::vec_of(with.clone());
        let other = self.built(&draft.stages, |inner| {
            inner.typed_only(|inner| inner.expr(&other_ty, depth - 1))
        });
        draft.push(Stage::Zip { other });
    }

    fn push_scan(&mut self, draft: &mut Draft, state: &Ty, depth: usize) {
        let acc = self.fresh_bind();
        let init = self.built(&draft.stages, |inner| {
            inner.typed_only(|inner| inner.expr(state, depth - 1))
        });
        let (bind, locals) = self.value_bind(&draft.item);
        // the state stays behind the `&mut`, so the body only reads it
        let behind = [(format!("(*{acc})"), state.clone())];
        let body = self.closure_body(|inner| {
            inner.with_borrowed(&behind, |inner| {
                inner.with_locals(&locals, |inner| {
                    inner.typed_only(|inner| inner.expr(state, depth - 1))
                })
            })
        });
        draft.push(Stage::Scan {
            acc: Bind::One(acc),
            bind,
            init,
            body,
        });
    }

    /// Any stage that fits the chain. A stage that makes a new item type picks a small one.
    fn free_stage(&mut self, draft: &mut Draft, depth: usize) {
        let shallow = draft.item.ty().depth() < MAX_TY_DEPTH;
        match self.rng.random_range(0..12) {
            0 | 1 => {
                let to = self.elem_ty();
                self.push_map(draft, &to, depth);
            }
            2 => {
                let to = self.scalar_ty();
                self.push_filter_map(draft, &to, depth);
            }
            3 => {
                let to = self.scalar_ty();
                self.push_flat_map(draft, &to, depth);
            }
            4 if shallow => {
                let with = self.scalar_ty();
                self.push_zip(draft, &with, depth);
            }
            5 if shallow && Stage::Enumerate.fits(&draft.item, draft.caps) => {
                draft.push(Stage::Enumerate);
            }
            6 => {
                let state = self.scalar_ty();
                self.push_scan(draft, &state, depth);
            }
            7 if Stage::Flatten.fits(&draft.item, draft.caps) => draft.push(Stage::Flatten),
            _ => self.keep_stage(draft, depth),
        }
    }

    /// One stage that keeps the item type, from the ones the chain allows.
    fn keep_stage(&mut self, draft: &mut Draft, depth: usize) {
        let mut options = vec![
            Keep::Filter,
            Keep::TakeWhile,
            Keep::SkipWhile,
            Keep::Take,
            Keep::StepBy,
            Keep::Peekable,
            Keep::Chain,
        ];
        if Stage::Rev.fits(&draft.item, draft.caps) {
            options.push(Keep::Rev);
        }
        if !draft.pending() {
            options.push(Keep::Skip);
        }
        if Stage::Inspect.fits(&draft.item, draft.caps) {
            options.push(Keep::Inspect);
        }
        if Stage::ByRefTake(0).fits(&draft.item, draft.caps) {
            options.push(Keep::ByRef);
        }
        let stage = match *self.pick(&options) {
            Keep::Filter => {
                let (bind, by, pred) = self.ref_closure(&draft.item, &Ty::Bool, depth - 1);
                let ann = self.param_ann();
                Stage::Filter {
                    bind,
                    pred,
                    ann,
                    by,
                }
            }
            Keep::TakeWhile => {
                let (bind, by, pred) = self.ref_closure(&draft.item, &Ty::Bool, depth - 1);
                Stage::TakeWhile { bind, pred, by }
            }
            Keep::SkipWhile => {
                let (bind, by, pred) = self.ref_closure(&draft.item, &Ty::Bool, depth - 1);
                Stage::SkipWhile { bind, pred, by }
            }
            Keep::Take => Stage::Take(self.rng.random_range(0..=5)),
            Keep::StepBy => Stage::StepBy(*self.pick(&[1u8, 1, 2, 2, 3, 0])),
            Keep::Peekable => Stage::Peekable,
            Keep::Chain => {
                let other_ty = Ty::vec_of(draft.item.ty());
                let other = self.built(&draft.stages, |inner| inner.expr(&other_ty, depth - 1));
                Stage::Chain { other }
            }
            Keep::Rev => Stage::Rev,
            Keep::Skip => Stage::Skip(self.rng.random_range(0..=3)),
            Keep::Inspect => Stage::Inspect,
            Keep::ByRef => Stage::ByRefTake(self.rng.random_range(0..=3)),
        };
        draft.push(stage);
    }

    /// Brings the item of the chain to `need`, through one of the stages that compute an item.
    fn adapt(&mut self, draft: &mut Draft, need: &Ty, depth: usize) {
        let deep = need.depth() >= MAX_TY_DEPTH;
        match self.rng.random_range(0..10) {
            0 if !deep => self.push_filter_map(draft, need, depth),
            1 if !deep => self.push_flat_map(draft, need, depth),
            2 if !deep => {
                let wrapped = if self.chance(0.5) {
                    Ty::vec_of(need.clone())
                } else {
                    Ty::opt_of(need.clone())
                };
                self.push_map(draft, &wrapped, depth);
                draft.push(Stage::Flatten);
            }
            3 => self.push_scan(draft, need, depth),
            4 | 5 => match need {
                Ty::Tuple(parts) if parts.len() == 2 => {
                    if draft.item.ty() != parts[0] {
                        self.push_map(draft, &parts[0], depth);
                    }
                    self.push_zip(draft, &parts[1], depth);
                }
                _ => self.push_map(draft, need, depth),
            },
            _ => self.push_map(draft, need, depth),
        }
    }

    fn end_term(
        &mut self,
        end: End,
        want: &Ty,
        site: Site,
        draft: &mut Draft,
        depth: usize,
    ) -> Option<Term> {
        let term = match end {
            End::CollectVec | End::CollectString | End::CollectResult => Term::Collect {
                target: want.clone(),
                site,
            },
            End::Unzip => Term::Unzip,
            End::ForEach => {
                let elem = want.elem().cloned()?;
                let (bind, body) = self.value_closure(&draft.item, &elem, depth - 1);
                Term::ForEach { bind, body }
            }
            End::Partition => {
                let (bind, by, pred) = self.ref_closure(&draft.item, &Ty::Bool, depth - 1);
                Term::Partition { bind, pred, by }
            }
            End::Find => {
                let (bind, by, pred) = self.ref_closure(&draft.item, &Ty::Bool, depth - 1);
                Term::Find { bind, pred, by }
            }
            End::FindMap => {
                let (bind, body) = self.value_closure(&draft.item, want, depth - 1);
                Term::FindMap { bind, body }
            }
            End::MaxByKey | End::MinByKey => {
                // a key that prints when it drops would show the order std compares in
                let key_ty = self
                    .pick(&[
                        Ty::I64,
                        Ty::Int(IntWidth::U8),
                        Ty::USIZE,
                        Ty::Bool,
                        Ty::Char,
                        Ty::Str,
                    ])
                    .clone();
                let (bind, by, key) = self.ref_closure(&draft.item, &key_ty, depth - 1);
                if matches!(end, End::MaxByKey) {
                    Term::MaxByKey { bind, key, by }
                } else {
                    Term::MinByKey { bind, key, by }
                }
            }
            End::Reduce => {
                let acc = self.fresh_bind();
                let bind = self.fresh_bind();
                let item = draft.item.ty();
                let locals = [(acc.clone(), item.clone()), (bind.clone(), item.clone())];
                let body = self.closure_body(|inner| {
                    inner.with_locals(&locals, |inner| {
                        inner.typed_only(|inner| inner.expr(&item, depth - 1))
                    })
                });
                Term::Reduce {
                    acc: Bind::One(acc),
                    bind: Bind::One(bind),
                    body,
                }
            }
            End::Rposition => {
                // a sorted vec runs from the back and knows its length
                if !(draft.caps.double_ended && draft.caps.exact_size) {
                    if !draft.item.is_ord() {
                        return None;
                    }
                    draft.push(Stage::Sorted);
                }
                let (bind, pred) = self.value_closure(&draft.item, &Ty::Bool, depth - 1);
                Term::Rposition { bind, pred }
            }
            End::Fold => self.fold_term(want, &draft.item.ty(), &draft.stages, depth),
        };
        Some(term)
    }
}
