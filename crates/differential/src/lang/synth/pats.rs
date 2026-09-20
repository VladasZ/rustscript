//! Pattern generation. A pattern is drawn by type, so a nested one appears wherever a payload,
//! a tuple slot or a field has room for it.

use rand::RngExt;

use crate::lang::own::BindKind;
use crate::lang::pat::{BindBy, Pat};
use crate::lang::synth::Generator;
use crate::lang::ty::{IntWidth, Ty};
use crate::lang::user::UserShape;

/// How deep a pattern nests under the arm's own constructor.
pub(super) const MAX_PAT_DEPTH: usize = 2;

impl Generator<'_> {
    pub(super) fn bind(&mut self, ty: &Ty) -> Pat {
        Pat::Bind {
            name: self.fresh("diff_b"),
            ty: ty.clone(),
            by: BindBy::Value,
        }
    }

    /// A plain binding most of the time, so the arm body has something to read.
    fn bind_or_wild(&mut self, ty: &Ty, by_ref: bool) -> Pat {
        match self.rng.random_range(0..10) {
            0 => Pat::Wild,
            // a slice view already hands out references
            1 | 2 if !by_ref => Pat::Bind {
                name: self.fresh("diff_b"),
                ty: ty.clone(),
                by: BindBy::Ref,
            },
            _ => self.bind(ty),
        }
    }

    /// A pattern for one slot of type `ty`. `by_ref` is set under a slice view, where a `ref`
    /// binding would bind a reference to a reference.
    pub(super) fn sub_pat(&mut self, ty: &Ty, depth: usize, by_ref: bool) -> Pat {
        if depth == 0 || self.chance(0.5) {
            return self.bind_or_wild(ty, by_ref);
        }
        match ty {
            Ty::Int(width) => self.int_pat(*width),
            Ty::Bool => Pat::BoolLit(self.chance(0.5)),
            Ty::Char => self.char_pat(),
            Ty::Opt(inner) => {
                if self.chance(0.7) {
                    Pat::Some(Box::new(self.sub_pat(inner, depth - 1, by_ref)))
                } else {
                    Pat::None
                }
            }
            Ty::Res(ok, err) => {
                if self.chance(0.6) {
                    Pat::Ok(Box::new(self.sub_pat(ok, depth - 1, by_ref)))
                } else {
                    Pat::Err(Box::new(self.sub_pat(err, depth - 1, by_ref)))
                }
            }
            Ty::Tuple(items) => Pat::Tuple(
                items
                    .iter()
                    .map(|item| self.sub_pat(item, depth - 1, by_ref))
                    .collect(),
            ),
            Ty::User(shape) if shape.is_enum() => {
                let variant = self.rng.random_range(0..shape.variants().len());
                self.variant_pat(shape, variant, depth - 1, by_ref)
            }
            Ty::User(shape) => self.struct_pat(shape, depth - 1, by_ref),
            _ => self.bind_or_wild(ty, by_ref),
        }
    }

    pub(super) fn variant_pat(
        &mut self,
        shape: &UserShape,
        variant: usize,
        depth: usize,
        by_ref: bool,
    ) -> Pat {
        let payload = shape.variants()[variant]
            .payload
            .iter()
            .map(|ty| self.sub_pat(ty, depth, by_ref))
            .collect();
        Pat::Variant {
            shape: Box::new(shape.clone()),
            variant,
            payload,
        }
    }

    pub(super) fn struct_pat(&mut self, shape: &UserShape, depth: usize, by_ref: bool) -> Pat {
        let mut fields: Vec<(usize, Pat)> = Vec::new();
        for (index, field) in shape.fields().iter().enumerate() {
            if self.chance(0.6) {
                fields.push((index, self.sub_pat(&field.ty, depth, by_ref)));
            }
        }
        Pat::Struct {
            shape: Box::new(shape.clone()),
            fields,
        }
    }

    /// A literal, a range, an or of them, a const, or any of those under `name @`.
    pub(super) fn int_pat(&mut self, width: IntWidth) -> Pat {
        let plain = match self.rng.random_range(0..5) {
            0 => self.int_lit_pat(width),
            2 => {
                let count = self.rng.random_range(2..=3);
                Pat::Or(
                    (0..count)
                        .map(|_| {
                            if self.chance(0.7) {
                                self.int_lit_pat(width)
                            } else {
                                self.int_range_pat(width)
                            }
                        })
                        .collect(),
                )
            }
            3 => self
                .const_pat(&Ty::Int(width))
                .unwrap_or_else(|| self.int_lit_pat(width)),
            _ => self.int_range_pat(width),
        };
        if self.chance(0.25) {
            return Pat::At {
                name: self.fresh("diff_b"),
                ty: Ty::Int(width),
                pat: Box::new(plain),
            };
        }
        plain
    }

    fn int_lit_pat(&mut self, width: IntWidth) -> Pat {
        Pat::IntLit {
            width,
            value: self.int_value(width),
        }
    }

    fn int_range_pat(&mut self, width: IntWidth) -> Pat {
        let a = self.int_value(width);
        let b = self.int_value(width);
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        // an empty half open range is a compile error
        let inclusive = lo == hi || self.chance(0.5);
        Pat::IntRange {
            width,
            lo,
            hi,
            inclusive,
        }
    }

    pub(super) fn char_pat(&mut self) -> Pat {
        match self.rng.random_range(0..4) {
            0 => {
                let a = self.char_value();
                let b = self.char_value();
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                Pat::CharRange { lo, hi }
            }
            1 => Pat::Or(vec![
                Pat::CharLit(self.char_value()),
                Pat::CharLit(self.char_value()),
            ]),
            2 => self
                .const_pat(&Ty::Char)
                .unwrap_or_else(|| Pat::CharLit(self.char_value())),
            _ => Pat::CharLit(self.char_value()),
        }
    }

    /// A const of the block as a pattern. A float const is no pattern, NaN never matches and
    /// `rustc` rejects it.
    fn const_pat(&mut self, ty: &Ty) -> Option<Pat> {
        let consts: Vec<String> = self
            .scope
            .visible()
            .into_iter()
            .filter(|slot| matches!(slot.kind, BindKind::Const) && slot.ty == *ty)
            .map(|slot| slot.name.clone())
            .collect();
        if consts.is_empty() {
            return None;
        }
        Some(Pat::Const {
            name: self.pick(&consts).clone(),
        })
    }

    /// A refutable pattern for `if let` and `matches!`, or `None` when the type has none.
    pub(super) fn refutable_pat(&mut self, ty: &Ty) -> Option<Pat> {
        self.refutable_pat_with(ty, false)
    }

    /// The same without `ref` bindings, for a `let else` whose scrutinee is a temporary.
    pub(super) fn refutable_value_pat(&mut self, ty: &Ty) -> Option<Pat> {
        self.refutable_pat_with(ty, true)
    }

    fn refutable_pat_with(&mut self, ty: &Ty, no_ref: bool) -> Option<Pat> {
        let pat = match ty {
            Ty::Opt(inner) => {
                if self.chance(0.85) {
                    Pat::Some(Box::new(self.sub_pat(inner, MAX_PAT_DEPTH - 1, no_ref)))
                } else {
                    Pat::None
                }
            }
            Ty::Res(ok, err) => {
                if self.chance(0.6) {
                    Pat::Ok(Box::new(self.sub_pat(ok, MAX_PAT_DEPTH - 1, no_ref)))
                } else {
                    Pat::Err(Box::new(self.sub_pat(err, MAX_PAT_DEPTH - 1, no_ref)))
                }
            }
            Ty::User(shape) if shape.is_enum() => {
                let variant = self.rng.random_range(0..shape.variants().len());
                self.variant_pat(shape, variant, MAX_PAT_DEPTH - 1, no_ref)
            }
            Ty::Int(width) => self.int_pat(*width),
            Ty::Char => self.char_pat(),
            Ty::Tuple(items) => {
                let pats: Vec<Pat> = items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| match item {
                        Ty::Int(width) if index == 0 => self.int_pat(*width),
                        Ty::Bool if index == 0 => Pat::BoolLit(self.chance(0.5)),
                        _ => self.sub_pat(item, MAX_PAT_DEPTH - 1, no_ref),
                    })
                    .collect();
                Pat::Tuple(pats)
            }
            _ => return None,
        };
        (!pat.is_irrefutable()).then_some(pat)
    }

    /// A scrutinee type a refutable pattern exists for.
    pub(super) fn refutable_ty(&mut self) -> Ty {
        match self.rng.random_range(0..8) {
            0..=2 => Ty::opt_of(self.elem_ty()),
            3 => self.res_ty(),
            4 => match self.user_ty() {
                Some(Ty::User(shape)) if shape.is_enum() => Ty::User(shape),
                _ => Ty::opt_of(self.scalar_ty()),
            },
            5 => Ty::Int(self.int_width()),
            6 => Ty::Char,
            _ => Ty::Tuple(vec![Ty::Int(self.int_width()), self.scalar_ty()]),
        }
    }

    /// Runs `build` with the bindings of `pat` in scope. A `ref` binding stands behind a
    /// reference, so the body reads it by clone alone.
    pub(super) fn with_pat<T>(&mut self, pat: &Pat, build: impl FnOnce(&mut Self) -> T) -> T {
        let mut binds = Vec::new();
        pat.bindings(&mut binds);
        let borrowed = pat.borrowed();
        self.scoped(|inner| {
            for (name, ty) in binds {
                if borrowed.contains(&name) {
                    inner.scope.push_borrowed(name, ty);
                } else {
                    inner.push_local(name, ty);
                }
            }
            build(inner)
        })
    }
}
