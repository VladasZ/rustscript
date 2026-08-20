//! `match` generation over options, results, user enums and structs,
//! integers with ranges and guards, booleans, tuples, and slices.

use rand::RngExt;

use crate::lang::expr::{Arm, Expr};
use crate::lang::pat::Pat;
use crate::lang::synth::Generator;
use crate::lang::ty::{IntWidth, Ty};
use crate::lang::user::UserShape;

impl Generator<'_> {
    /// A match whose every arm produces `want`.
    pub(super) fn match_expr(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let scrutinee_ty = match self.rng.random_range(0..8) {
            0 => Ty::opt_of(self.elem_ty()),
            1 => self.res_ty(),
            2 | 3 => self.user_ty()?,
            4 => Ty::Int(self.int_width()),
            5 => Ty::Bool,
            6 => Ty::Tuple(vec![self.scalar_ty(), self.scalar_ty()]),
            _ => Ty::vec_of(self.elem_ty()),
        };
        let scrutinee = self.expr(&scrutinee_ty, depth - 1);
        let by_ref = matches!(scrutinee_ty, Ty::Vec(_));
        let arms = match &scrutinee_ty {
            Ty::Opt(inner) => self.option_arms(inner, want, depth),
            Ty::Res(ok, err) => self.result_arms(ok, err, want, depth),
            Ty::User(shape) if shape.is_enum() => self.enum_arms(shape, want, depth),
            Ty::User(shape) => self.struct_arms(shape, want, depth),
            Ty::Int(width) => self.int_arms(*width, want, depth),
            Ty::Bool => self.bool_arms(want, depth),
            Ty::Tuple(items) => self.tuple_arms(items, want, depth),
            Ty::Vec(elem) => self.slice_arms(elem, want, depth),
            _ => return None,
        };
        Some(Expr::Match {
            scrutinee: Box::new(scrutinee),
            by_ref,
            arms,
            ty: want.clone(),
        })
    }

    /// An arm with `binds` in scope for the guard and the body.
    fn arm(&mut self, pat: Pat, guard: bool, want: &Ty, depth: usize) -> Arm {
        let mut binds = Vec::new();
        pat.bindings(&mut binds);
        self.with_locals(&binds, |inner| {
            let guard = guard.then(|| inner.expr(&Ty::Bool, depth - 1));
            let body = inner.expr(want, depth - 1);
            Arm { pat, guard, body }
        })
    }

    fn wild_arm(&mut self, want: &Ty, depth: usize) -> Arm {
        Arm {
            pat: Pat::Wild,
            guard: None,
            body: self.expr(want, depth - 1),
        }
    }

    fn bind(&mut self, ty: &Ty) -> Pat {
        Pat::Bind {
            name: self.fresh("diff_b"),
            ty: ty.clone(),
        }
    }

    fn option_arms(&mut self, inner: &Ty, want: &Ty, depth: usize) -> Vec<Arm> {
        let guarded = self.chance(0.3);
        let some_pat = Pat::Some(Box::new(self.bind(inner)));
        let mut arms = vec![self.arm(some_pat, guarded, want, depth)];
        arms.push(self.arm(Pat::None, false, want, depth));
        if guarded {
            arms.push(self.wild_arm(want, depth));
        }
        arms
    }

    fn result_arms(&mut self, ok: &Ty, err: &Ty, want: &Ty, depth: usize) -> Vec<Arm> {
        let ok_pat = Pat::Ok(Box::new(self.bind(ok)));
        let err_pat = Pat::Err(Box::new(self.bind(err)));
        vec![
            self.arm(ok_pat, false, want, depth),
            self.arm(err_pat, false, want, depth),
        ]
    }

    fn enum_arms(&mut self, shape: &UserShape, want: &Ty, depth: usize) -> Vec<Arm> {
        let mut arms = Vec::new();
        let skip_some = self.chance(0.3);
        let mut skipped = false;
        for (index, variant) in shape.variants().iter().enumerate() {
            if skip_some && index > 0 && self.chance(0.4) {
                skipped = true;
                continue;
            }
            let payload = variant
                .payload
                .iter()
                .map(|ty| {
                    if self.chance(0.2) {
                        Pat::Wild
                    } else {
                        self.bind(ty)
                    }
                })
                .collect();
            let pat = Pat::Variant {
                shape: Box::new(shape.clone()),
                variant: index,
                payload,
            };
            arms.push(self.arm(pat, false, want, depth));
        }
        if skipped || arms.is_empty() {
            arms.push(self.wild_arm(want, depth));
        }
        arms
    }

    /// One irrefutable struct pattern binding a subset of the fields.
    fn struct_arms(&mut self, shape: &UserShape, want: &Ty, depth: usize) -> Vec<Arm> {
        let mut fields: Vec<(usize, Pat)> = Vec::new();
        for (index, field) in shape.fields().iter().enumerate() {
            if self.chance(0.6) {
                let pat = self.bind(&field.ty);
                fields.push((index, pat));
            }
        }
        let pat = Pat::Struct {
            shape: Box::new(shape.clone()),
            fields,
        };
        vec![self.arm(pat, false, want, depth)]
    }

    fn int_arms(&mut self, width: IntWidth, want: &Ty, depth: usize) -> Vec<Arm> {
        let mut arms = Vec::new();
        let count = self.rng.random_range(1..=3);
        for _ in 0..count {
            let pat = match self.rng.random_range(0..3) {
                0 => Pat::IntLit {
                    width,
                    value: self.int_value(width),
                },
                1 => {
                    let a = self.int_value(width);
                    let b = self.int_value(width);
                    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                    // An empty half-open range is a compile error.
                    let inclusive = lo == hi || self.chance(0.5);
                    Pat::IntRange {
                        width,
                        lo,
                        hi,
                        inclusive,
                    }
                }
                _ => self.bind(&Ty::Int(width)),
            };
            let guarded = matches!(pat, Pat::Bind { .. }) || self.chance(0.2);
            arms.push(self.arm(pat, guarded, want, depth));
        }
        arms.push(self.wild_arm(want, depth));
        arms
    }

    fn bool_arms(&mut self, want: &Ty, depth: usize) -> Vec<Arm> {
        vec![
            self.arm(Pat::BoolLit(true), false, want, depth),
            self.arm(Pat::BoolLit(false), false, want, depth),
        ]
    }

    fn tuple_arms(&mut self, items: &[Ty], want: &Ty, depth: usize) -> Vec<Arm> {
        let mut arms = Vec::new();
        if self.chance(0.5) {
            // A literal in one slot, so the arm can miss.
            let pats: Vec<Pat> = items
                .iter()
                .enumerate()
                .map(|(index, ty)| match ty {
                    Ty::Int(width) if index == 0 => Pat::IntLit {
                        width: *width,
                        value: self.int_value(*width),
                    },
                    Ty::Bool if index == 0 => Pat::BoolLit(self.chance(0.5)),
                    _ => self.bind(ty),
                })
                .collect();
            let refutable = !pats.iter().all(Pat::is_irrefutable);
            arms.push(self.arm(Pat::Tuple(pats), false, want, depth));
            if !refutable {
                return arms;
            }
        }
        let pats: Vec<Pat> = items.iter().map(|ty| self.bind(ty)).collect();
        arms.push(self.arm(Pat::Tuple(pats), false, want, depth));
        arms
    }

    fn slice_arms(&mut self, elem: &Ty, want: &Ty, depth: usize) -> Vec<Arm> {
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
            // Slice bindings are references until the arm body clones
            // them, so a guard would see `&T`.
            arms.push(self.arm(pat, false, want, depth));
        }
        arms.push(self.wild_arm(want, depth));
        arms
    }
}
