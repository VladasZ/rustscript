//! Generation of `&str`, `&[T]` and the values that hold one. What a reference may borrow
//! depends on where it goes, see `RefRules`. The checker replays the same rules.

use rand::RngExt;

use crate::lang::catalog::{opt_str_pair, opt_str_ref};
use crate::lang::expr::{BorrowKind, Expr};
use crate::lang::own::{BindKind, OwnState, RefRules};
use crate::lang::stmt::Stmt;
use crate::lang::synth::{Generator, MAX_EXPR_DEPTH};
use crate::lang::ty::Ty;

impl Generator<'_> {
    /// A type with a reference in it, for a `let`, a print or a comparison.
    pub(super) fn ref_ty(&mut self) -> Ty {
        match self.rng.random_range(0..20) {
            0..=10 => Ty::StrRef,
            11..=14 => Ty::slice_of(self.elem_ty()),
            15..=17 => opt_str_ref(),
            _ => opt_str_pair(),
        }
    }

    /// `let r = s.as_str();`. The binding it borrows stays held until the scope ends, and the
    /// new binding is never written, so what it borrows never changes.
    pub(super) fn ref_binding(&mut self) -> Stmt {
        let ty = self.ref_ty();
        let name = self.fresh("v");
        let expr = self.kept(|inner| inner.expr(&ty, MAX_EXPR_DEPTH));
        let ann = self.ann_for(&expr);
        self.push_local(name.clone(), ty.clone());
        Stmt::Let {
            name,
            ty,
            expr,
            ann,
            mutable: false,
        }
    }

    /// Builds a value the `let` being built keeps.
    pub(super) fn kept<T>(&mut self, build: impl FnOnce(&mut Self) -> T) -> T {
        let saved = std::mem::replace(&mut self.refs, RefRules::kept(self.scope.depth()));
        let out = build(self);
        self.refs = saved;
        out
    }

    /// Builds a value that leaves an `if` branch or a match arm. The scope it leaves is not
    /// entered yet, so the depth here is the one it leaves to.
    fn leaving<T>(&mut self, build: impl FnOnce(&mut Self) -> T) -> T {
        let saved = self.refs;
        self.refs = saved.leaving(self.scope.depth());
        let out = build(self);
        self.refs = saved;
        out
    }

    pub(super) fn ref_expr(&mut self, want: &Ty, depth: usize) -> Expr {
        match want {
            Ty::StrRef | Ty::Slice(_) => self.reference(want, depth),
            _ => self.ref_holder(want, depth),
        }
    }

    /// The owned type a reference of type `want` borrows from.
    fn owner_of(want: &Ty) -> Ty {
        match want {
            Ty::Slice(elem) => Ty::vec_of((**elem).clone()),
            _ => Ty::Str,
        }
    }

    fn reference(&mut self, want: &Ty, depth: usize) -> Expr {
        let owner = Self::owner_of(want);
        for _ in 0..4 {
            let attempt = match self.rng.random_range(0..100) {
                0..=19 => Some(self.leaf(want)),
                20..=54 => self.place_borrow(&owner),
                55..=64 if depth > 0 => self.temp_borrow(&owner, depth),
                65..=72 if depth > 0 => Some(self.sub_borrow(want, depth)),
                73..=89 if depth > 0 => self.call(want, depth),
                90..=94 if depth > 0 => Some(self.leaving(|inner| inner.if_expr(want, depth))),
                95..=97 if depth > 0 => self.leaving(|inner| inner.match_expr(want, depth)),
                98..=99 if depth > 0 => Some(self.pick_call(want, depth)),
                _ => None,
            };
            if let Some(expr) = attempt {
                return expr;
            }
        }
        self.leaf(want)
    }

    /// `Option<&str>` and the like, from a call that hands one out, a binding or a literal.
    fn ref_holder(&mut self, want: &Ty, depth: usize) -> Expr {
        let below = depth.saturating_sub(1);
        if depth > 0 {
            match self.rng.random_range(0..10) {
                0..=5 => {
                    if let Some(expr) = self.call(want, depth) {
                        return expr;
                    }
                }
                6 => return self.leaving(|inner| inner.if_expr(want, depth)),
                _ => {}
            }
        }
        let locals = self.locals_of(want);
        if !locals.is_empty() && self.chance(0.4) {
            let name = self.pick(&locals).clone();
            return self.read(name, want);
        }
        match want {
            Ty::Opt(inner) => Expr::OptLit {
                elem: (**inner).clone(),
                value: self.chance(0.75).then(|| Box::new(self.expr(inner, below))),
            },
            Ty::Tuple(items) => {
                Expr::TupleLit(items.iter().map(|item| self.expr(item, below)).collect())
            }
            other => self.literal(other),
        }
    }

    /// `s.as_str()` or `&s[1..]` over a live `let`, which stays borrowed for as long as the
    /// reference lives.
    fn place_borrow(&mut self, owner: &Ty) -> Option<Expr> {
        let places: Vec<String> = self
            .scope
            .visible()
            .into_iter()
            .filter(|slot| {
                matches!(slot.kind, BindKind::Local)
                    && slot.state == OwnState::Owned
                    && slot.ty == *owner
                    && self.scope.can_borrow(&slot.name, self.refs.floor)
            })
            .map(|slot| slot.name.clone())
            .collect();
        if places.is_empty() {
            return None;
        }
        let name = self.pick(&places).clone();
        self.scope.lend(&name, self.refs);
        Some(Expr::Borrow {
            base: Box::new(Expr::Var {
                name,
                ty: owner.clone(),
                mode: crate::lang::expr::ReadMode::Clone,
            }),
            kind: self.borrow_kind(),
        })
    }

    /// A borrow of a value no binding holds, `String::from("a").as_str()`. It ends with its
    /// statement, so the reference must be used up before that.
    fn temp_borrow(&mut self, owner: &Ty, depth: usize) -> Option<Expr> {
        if !self.refs.temps_ok() {
            return None;
        }
        // a read of a binding would render as the binding itself, which is a place borrow
        let base = match self.call(owner, depth) {
            Some(expr) => expr,
            None => self.literal(owner),
        };
        if matches!(base, Expr::Var { .. }) {
            return None;
        }
        Some(Expr::Borrow {
            base: Box::new(base),
            kind: self.borrow_kind(),
        })
    }

    /// `&r[..2]`, a part of another reference.
    fn sub_borrow(&mut self, want: &Ty, depth: usize) -> Expr {
        let base = self.expr(want, depth - 1);
        Expr::Borrow {
            base: Box::new(base),
            kind: self.range_kind(),
        }
    }

    /// `diff_pick("a", s.as_str(), true)`, both references through one generic parameter.
    fn pick_call(&mut self, want: &Ty, depth: usize) -> Expr {
        let name = self.generic_pick_fn();
        let first = self.expr(want, depth - 1);
        let second = self.expr(want, depth - 1);
        let flag = self.expr(&Ty::Bool, depth - 1);
        Expr::FnCall {
            name,
            args: vec![first, second, flag],
            by_ref: Vec::new(),
            ty: want.clone(),
        }
    }

    fn borrow_kind(&mut self) -> BorrowKind {
        if self.chance(0.75) {
            BorrowKind::Whole
        } else {
            self.range_kind()
        }
    }

    /// Small bounds, so a range often fits and now and then runs past the end or lands inside
    /// a char.
    fn range_kind(&mut self) -> BorrowKind {
        let (lo, hi) = match self.rng.random_range(0..4) {
            0 => (Some(self.rng.random_range(0..=2)), None),
            1 => (None, Some(self.rng.random_range(0..=3))),
            2 => (None, None),
            _ => {
                let lo = self.rng.random_range(0..=2);
                (Some(lo), Some(lo + self.rng.random_range(0..=2)))
            }
        };
        BorrowKind::Range { lo, hi }
    }

    /// The other spelling of a whole borrow where the position allows one, `&s` as an argument
    /// and a bare `s` as a receiver.
    pub(super) fn respell(&mut self, expr: Expr, kind: BorrowKind) -> Expr {
        match expr {
            Expr::Borrow {
                base,
                kind: BorrowKind::Whole,
            } if self.chance(0.6) => Expr::Borrow { base, kind },
            other => other,
        }
    }
}
