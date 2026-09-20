//! `match` generation.

use rand::RngExt;

use crate::lang::expr::{Arm, Expr, unbare_deep};
use crate::lang::pat::Pat;
use crate::lang::synth::Generator;
use crate::lang::synth::pats::MAX_PAT_DEPTH;
use crate::lang::ty::{IntWidth, Ty};
use crate::lang::user::UserShape;

impl Generator<'_> {
    pub(super) fn match_expr(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let scrutinee_ty = self.scrutinee_ty()?;
        let scrutinee = self.expr(&scrutinee_ty, depth - 1);
        let by_ref = matches!(scrutinee_ty, Ty::Vec(_));
        self.begin_branches();
        let arms = self.arms_for(&scrutinee_ty, &mut |inner, pat, guard| {
            inner.arm(pat, guard, want, depth)
        });
        self.end_branches();
        let arms = arms?;
        // An arm body may call a width specific method on a bound name, and `rustc` resolves the
        // method before the scrutinee's bare literals default, so a binding forces real suffixes.
        let mut binds = Vec::new();
        for arm in &arms {
            arm.pat.bindings(&mut binds);
        }
        let scrutinee = if binds.is_empty() {
            scrutinee
        } else {
            unbare_deep(scrutinee)
        };
        Some(Expr::Match {
            scrutinee: Box::new(scrutinee),
            by_ref,
            arms,
            ty: want.clone(),
        })
    }

    /// `matches!` over a refutable pattern. A guard is the only reader of the bindings, and it
    /// moves nothing, like a match guard.
    pub(super) fn matches_expr(&mut self, depth: usize) -> Option<Expr> {
        let ty = self.refutable_ty();
        let pat = self.refutable_value_pat(&ty)?;
        let scrutinee = unbare_deep(self.expr(&ty, depth - 1));
        let mut binds = Vec::new();
        pat.bindings(&mut binds);
        let guard = (!binds.is_empty() && self.chance(0.6)).then(|| {
            Box::new(self.with_pat(&pat.clone(), |inner| {
                inner.borrowing(|inner| inner.expr(&Ty::Bool, depth - 1))
            }))
        });
        Some(Expr::Matches {
            scrutinee: Box::new(scrutinee),
            pat,
            guard,
        })
    }

    /// A type some arm list below covers.
    pub(super) fn scrutinee_ty(&mut self) -> Option<Ty> {
        Some(match self.rng.random_range(0..9) {
            0 => Ty::opt_of(self.elem_ty()),
            1 => self.res_ty(),
            2 | 3 => self.user_ty()?,
            4 => Ty::Int(self.int_width()),
            5 => Ty::Bool,
            6 => Ty::Tuple(vec![self.scalar_ty(), self.scalar_ty()]),
            7 => Ty::Char,
            _ => Ty::vec_of(self.elem_ty()),
        })
    }

    /// The patterns of a `match` over `ty`, in order and exhaustive. `make` builds one arm
    /// from a pattern and whether it carries a guard, so a `match` expression and a `match`
    /// statement share the patterns.
    pub(super) fn arms_for<A>(
        &mut self,
        ty: &Ty,
        make: &mut impl FnMut(&mut Self, Pat, bool) -> A,
    ) -> Option<Vec<A>> {
        let mut arms = Vec::new();
        // set when the arms so far may all miss
        let mut open = false;
        match ty {
            Ty::Opt(inner) => {
                let guarded = self.chance(0.3);
                let payload = self.sub_pat(inner, MAX_PAT_DEPTH - 1, false);
                open = guarded || !payload.is_irrefutable();
                arms.push(make(self, Pat::Some(Box::new(payload)), guarded));
                arms.push(make(self, Pat::None, false));
            }
            Ty::Res(ok, err) => {
                let ok_pat = self.sub_pat(ok, MAX_PAT_DEPTH - 1, false);
                let err_pat = self.sub_pat(err, MAX_PAT_DEPTH - 1, false);
                open = !ok_pat.is_irrefutable() || !err_pat.is_irrefutable();
                arms.push(make(self, Pat::Ok(Box::new(ok_pat)), false));
                arms.push(make(self, Pat::Err(Box::new(err_pat)), false));
            }
            Ty::User(shape) if shape.is_enum() => {
                open = self.enum_pats(shape, &mut arms, make);
            }
            Ty::User(shape) => {
                let pat = self.struct_pat(shape, MAX_PAT_DEPTH - 1, false);
                open = !pat.is_irrefutable();
                arms.push(make(self, pat, false));
            }
            Ty::Int(width) => {
                self.int_pats(*width, &mut arms, make);
                open = true;
            }
            Ty::Char => {
                let count = self.rng.random_range(1..=3);
                for _ in 0..count {
                    let pat = self.char_pat();
                    arms.push(make(self, pat, false));
                }
                open = true;
            }
            Ty::Bool => {
                arms.push(make(self, Pat::BoolLit(true), false));
                arms.push(make(self, Pat::BoolLit(false), false));
            }
            Ty::Tuple(items) => {
                if self.chance(0.5) {
                    // a literal in 1 slot, so the arm can miss
                    let pats: Vec<Pat> = items
                        .iter()
                        .enumerate()
                        .map(|(index, item)| match item {
                            Ty::Int(width) if index == 0 => self.int_pat(*width),
                            Ty::Bool if index == 0 => Pat::BoolLit(self.chance(0.5)),
                            _ => self.sub_pat(item, MAX_PAT_DEPTH - 1, false),
                        })
                        .collect();
                    arms.push(make(self, Pat::Tuple(pats), false));
                }
                let pats: Vec<Pat> = items.iter().map(|item| self.bind(item)).collect();
                arms.push(make(self, Pat::Tuple(pats), false));
            }
            Ty::Vec(elem) => {
                for pat in self.slice_pats(elem) {
                    // a guard would see `&T`, the arm body clones later
                    arms.push(make(self, pat, false));
                }
                open = true;
            }
            _ => return None,
        }
        if open {
            arms.push(make(self, Pat::Wild, false));
        }
        Some(arms)
    }

    /// Whether some variant is left uncovered.
    fn enum_pats<A>(
        &mut self,
        shape: &UserShape,
        arms: &mut Vec<A>,
        make: &mut impl FnMut(&mut Self, Pat, bool) -> A,
    ) -> bool {
        let skip_some = self.chance(0.3);
        let mut open = false;
        let mut index = 0;
        let count = shape.variants().len();
        while index < count {
            if skip_some && index > 0 && self.chance(0.4) {
                open = true;
                index += 1;
                continue;
            }
            // 2 neighbours share an arm through an or pattern that binds nothing
            if index + 1 < count && self.chance(0.2) {
                let alternatives = (index..=index + 1)
                    .map(|variant| Pat::Variant {
                        shape: Box::new(shape.clone()),
                        variant,
                        payload: shape.variants()[variant]
                            .payload
                            .iter()
                            .map(|_| Pat::Wild)
                            .collect(),
                    })
                    .collect();
                arms.push(make(self, Pat::Or(alternatives), false));
                index += 2;
                continue;
            }
            let pat = self.variant_pat(shape, index, MAX_PAT_DEPTH - 1, false);
            open |= !pat.is_irrefutable_payload();
            arms.push(make(self, pat, false));
            index += 1;
        }
        open || arms.is_empty()
    }

    fn int_pats<A>(
        &mut self,
        width: IntWidth,
        arms: &mut Vec<A>,
        make: &mut impl FnMut(&mut Self, Pat, bool) -> A,
    ) {
        let count = self.rng.random_range(1..=3);
        for _ in 0..count {
            let pat = if self.chance(0.3) {
                self.bind(&Ty::Int(width))
            } else {
                self.int_pat(width)
            };
            let guarded = matches!(pat, Pat::Bind { .. }) || self.chance(0.2);
            arms.push(make(self, pat, guarded));
        }
    }

    fn arm(&mut self, pat: Pat, guard: bool, want: &Ty, depth: usize) -> Arm {
        self.branch(|inner| {
            inner.with_pat(&pat.clone(), |inner| {
                // a guard runs with the scrutinee borrowed and may run for several arms, so
                // `rustc` lets it move nothing, a binding or an outer local alike
                let guard =
                    guard.then(|| inner.borrowing(|inner| inner.expr(&Ty::Bool, depth - 1)));
                let body = inner.expr(want, depth - 1);
                Arm { pat, guard, body }
            })
        })
    }

    fn slice_pats(&mut self, elem: &Ty) -> Vec<Pat> {
        let mut arms = Vec::new();
        let shapes = self.rng.random_range(1..=3);
        for _ in 0..shapes {
            let pat = match self.rng.random_range(0..6) {
                0 => Pat::Slice {
                    elem: elem.clone(),
                    prefix: Vec::new(),
                    rest: None,
                    suffix: Vec::new(),
                },
                1 => Pat::Slice {
                    elem: elem.clone(),
                    prefix: vec![self.bind(elem)],
                    rest: None,
                    suffix: Vec::new(),
                },
                2 => Pat::Slice {
                    elem: elem.clone(),
                    prefix: vec![self.bind(elem)],
                    rest: Some(None),
                    suffix: vec![self.bind(elem)],
                },
                3 => Pat::Slice {
                    elem: elem.clone(),
                    prefix: vec![self.bind(elem)],
                    rest: Some(Some(self.fresh("diff_rest"))),
                    suffix: Vec::new(),
                },
                4 => Pat::Slice {
                    elem: elem.clone(),
                    prefix: Vec::new(),
                    rest: Some(Some(self.fresh("diff_rest"))),
                    suffix: vec![self.bind(elem)],
                },
                _ => Pat::Slice {
                    elem: elem.clone(),
                    prefix: vec![self.bind(elem), self.bind(elem)],
                    rest: Some(None),
                    suffix: Vec::new(),
                },
            };
            arms.push(pat);
        }
        arms
    }
}
