//! Type directed generation. The generator is asked for an expression of a
//! type and answers with any shape that produces it: a binding, a literal, an
//! operator, a cast, a branch, a match, a field, a user method, a `?`, or a
//! catalog method whose result unifies with the wanted type.
//!
//! Because every shape is chosen by type rather than by a hand written case,
//! a method added to the catalog immediately appears inside conditions, inside
//! loop bodies, as a receiver of another call, and at any depth.

mod exprs;
mod matches;
mod pipes;
mod stmts;
mod users;

use rand::RngExt;
use rand::rngs::StdRng;

use crate::lang::block::{Block, ConstDef, FnDef};
use crate::lang::expr::Expr;
use crate::lang::ty::{
    FLOAT_WIDTHS, FloatWidth, INT_WIDTHS, IntWidth, MAX_TY_DEPTH, SCALAR_TYPES, StdErr, Ty,
};
use crate::lang::user::UserDef;

/// How deep one expression may nest. Enough for a call whose receiver is a
/// call whose argument is an operator, which is where composition bugs live.
pub(super) const MAX_EXPR_DEPTH: usize = 3;

/// What a name in scope is.
#[derive(Clone, Debug)]
pub(super) enum BindKind {
    Local,
    Const,
    Closure { params: Vec<Ty>, ret: Ty },
}

#[derive(Clone, Debug)]
pub(super) struct Binding {
    pub(super) name: String,
    pub(super) ty: Ty,
    pub(super) kind: BindKind,
}

pub struct Generator<'a> {
    pub(super) rng: &'a mut StdRng,
    pub(super) scope: Vec<Binding>,
    pub(super) labels: usize,
    /// The block's index within its program, baked into every top level
    /// item name so two blocks never define the same item.
    pub(super) tag: usize,
    pub(super) types: Vec<UserDef>,
    pub(super) fns: Vec<FnDef>,
    pub(super) consts: Vec<ConstDef>,
    pub(super) describes: Vec<Ty>,
    /// The return type of the function body being generated, which is what
    /// lets `?` and an early `return` appear.
    pub(super) fn_ret: Option<Ty>,
    /// Inside a loop body, so `break` and `continue` may appear.
    pub(super) in_loop: bool,
    /// Inside a method receiver, where a bare literal would leave rustc
    /// with an ambiguous `{integer}` to call the method on.
    pub(super) forbid_bare: bool,
}

impl<'a> Generator<'a> {
    pub fn new(rng: &'a mut StdRng, tag: usize) -> Self {
        Self {
            rng,
            scope: Vec::new(),
            labels: 0,
            tag,
            types: Vec::new(),
            fns: Vec::new(),
            consts: Vec::new(),
            describes: Vec::new(),
            fn_ret: None,
            in_loop: false,
            forbid_bare: false,
        }
    }

    /// Generate a closure body. A `?`, a `break`, a `continue`, or a
    /// `return` inside it would answer to the closure, not to the function
    /// or loop around it, so neither is offered there.
    pub(super) fn closure_body<T>(&mut self, build: impl FnOnce(&mut Self) -> T) -> T {
        let saved_ret = self.fn_ret.take();
        let saved_loop = std::mem::replace(&mut self.in_loop, false);
        let out = build(self);
        self.fn_ret = saved_ret;
        self.in_loop = saved_loop;
        out
    }

    /// Generate a receiver position, where no bare literal may sit.
    pub(super) fn typed_only<T>(&mut self, build: impl FnOnce(&mut Self) -> T) -> T {
        let was = std::mem::replace(&mut self.forbid_bare, true);
        let out = build(self);
        self.forbid_bare = was;
        out
    }

    pub fn block(&mut self) -> Block {
        self.declare_types();
        self.declare_consts();
        self.declare_describes();
        let mut statements = Vec::new();
        let bindings = self.rng.random_range(3..=6);
        for _ in 0..bindings {
            statements.push(self.binding_stmt());
        }
        let extras = self.rng.random_range(3..=7);
        for _ in 0..extras {
            statements.push(self.mutation());
        }
        // Every binding is observed, then a few free standing expressions, so
        // a divergence in a value that was never stored still shows up.
        let observed: Vec<(String, Ty)> = self
            .scope
            .iter()
            .filter(|binding| matches!(binding.kind, BindKind::Local))
            .map(|binding| (binding.name.clone(), binding.ty.clone()))
            .collect();
        for (name, ty) in observed {
            statements.push(self.print_stmt(Expr::Var { name, ty }));
        }
        let observations = self.rng.random_range(2..=4);
        for _ in 0..observations {
            let ty = self.any_ty();
            let expr = self.expr(&ty, MAX_EXPR_DEPTH);
            statements.push(self.print_stmt(expr));
        }
        let mut block = Block {
            statements,
            fns: std::mem::take(&mut self.fns),
            consts: std::mem::take(&mut self.consts),
            types: std::mem::take(&mut self.types),
            describes: std::mem::take(&mut self.describes),
        };
        block.fix_apply_borrows();
        block.seal();
        block
    }

    // -- names and draws ------------------------------------------------------

    pub(super) fn fresh(&mut self, prefix: &str) -> String {
        let name = format!("{prefix}_{}_{}", self.tag, self.labels);
        self.labels += 1;
        name
    }

    pub(super) fn next_label(&mut self) -> String {
        self.fresh("lang")
    }

    pub(super) fn chance(&mut self, p: f64) -> bool {
        self.rng.random_bool(p)
    }

    pub(super) fn pick<'s, T>(&mut self, items: &'s [T]) -> &'s T {
        &items[self.rng.random_range(0..items.len())]
    }

    /// Locals of exactly this type.
    pub(super) fn locals_of(&self, want: &Ty) -> Vec<String> {
        self.scope
            .iter()
            .filter(|binding| matches!(binding.kind, BindKind::Local) && binding.ty == *want)
            .map(|binding| binding.name.clone())
            .collect()
    }

    pub(super) fn push_local(&mut self, name: String, ty: Ty) {
        self.scope.push(Binding {
            name,
            ty,
            kind: BindKind::Local,
        });
    }

    /// Generate with extra locals visible, then forget them.
    pub(super) fn with_locals<T>(
        &mut self,
        locals: &[(String, Ty)],
        build: impl FnOnce(&mut Self) -> T,
    ) -> T {
        for (name, ty) in locals {
            self.push_local(name.clone(), ty.clone());
        }
        let out = build(self);
        for _ in locals {
            self.scope.pop();
        }
        out
    }

    /// Generate with one binding hidden, for the argument of a call that
    /// already borrows that binding mutably.
    pub(super) fn without_binding<T>(
        &mut self,
        name: &str,
        build: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let index = self
            .scope
            .iter()
            .position(|binding| binding.name == name)
            .expect("the hidden binding is in scope");
        let hidden = self.scope.remove(index);
        let out = build(self);
        self.scope.insert(index, hidden);
        out
    }

    /// Generate with the whole scope hidden, for a top level item body.
    pub(super) fn without_scope<T>(&mut self, build: impl FnOnce(&mut Self) -> T) -> T {
        let saved = std::mem::take(&mut self.scope);
        let saved_loop = std::mem::replace(&mut self.in_loop, false);
        let out = build(self);
        self.scope = saved;
        self.in_loop = saved_loop;
        out
    }

    // -- types --------------------------------------------------------------

    pub(super) fn any_ty(&mut self) -> Ty {
        match self.rng.random_range(0..16) {
            0 | 1 => Ty::vec_of(self.elem_ty()),
            2 => Ty::opt_of(self.elem_ty()),
            3 => self.map_ty(),
            4 => self.set_ty(),
            5 => self.tuple_ty(),
            6 => self.res_ty(),
            7 | 8 => self.user_ty().unwrap_or_else(|| self.scalar_ty()),
            _ => self.scalar_ty(),
        }
    }

    /// An element type for a container: a scalar, a small tuple, a user
    /// type, or another container one level down.
    pub(super) fn elem_ty(&mut self) -> Ty {
        match self.rng.random_range(0..10) {
            0 => self.tuple_ty(),
            1 => self.user_ty().unwrap_or_else(|| self.scalar_ty()),
            2 => Ty::vec_of(self.scalar_ty()),
            3 => Ty::opt_of(self.scalar_ty()),
            _ => self.scalar_ty(),
        }
    }

    /// A key for a map or set: hashable, `Eq`, and ordered.
    pub(super) fn key_ty(&mut self) -> Ty {
        for _ in 0..8 {
            let candidate = match self.rng.random_range(0..8) {
                0 => Ty::Tuple(vec![self.scalar_ty(), self.scalar_ty()]),
                1 => match self.user_ty() {
                    Some(ty) => ty,
                    None => self.scalar_ty(),
                },
                2 => Ty::opt_of(self.scalar_ty()),
                _ => self.scalar_ty(),
            };
            if candidate.is_key() && candidate.depth() <= MAX_TY_DEPTH {
                return candidate;
            }
        }
        Ty::I64
    }

    /// A map value: ordered and defaultable, see `is_map_val`.
    pub(super) fn val_ty(&mut self) -> Ty {
        for _ in 0..8 {
            let candidate = match self.rng.random_range(0..8) {
                0 => Ty::vec_of(self.scalar_ty()),
                1 => Ty::opt_of(self.scalar_ty()),
                2 => Ty::Tuple(vec![self.scalar_ty(), self.scalar_ty()]),
                3 => match self.user_ty() {
                    Some(ty) => ty,
                    None => self.scalar_ty(),
                },
                _ => self.scalar_ty(),
            };
            if crate::lang::catalog::is_map_val(&candidate) && candidate.depth() <= MAX_TY_DEPTH {
                return candidate;
            }
        }
        Ty::I64
    }

    pub(super) fn map_ty(&mut self) -> Ty {
        let key = self.key_ty();
        let value = self.val_ty();
        Ty::map_of(key, value)
    }

    pub(super) fn set_ty(&mut self) -> Ty {
        Ty::set_of(self.key_ty())
    }

    pub(super) fn tuple_ty(&mut self) -> Ty {
        let count = self.rng.random_range(1..=3);
        let items = (0..count)
            .map(|_| {
                if self.chance(0.2) {
                    Ty::opt_of(self.scalar_ty())
                } else {
                    self.scalar_ty()
                }
            })
            .collect();
        Ty::Tuple(items)
    }

    /// `Result<T, E>` with a user error enum when the block declares one,
    /// a std parse error or a `String` otherwise.
    pub(super) fn res_ty(&mut self) -> Ty {
        let ok = if self.chance(0.3) {
            Ty::vec_of(self.scalar_ty())
        } else {
            self.scalar_ty()
        };
        let err = match self.rng.random_range(0..4) {
            0 => Ty::Str,
            1 => Ty::StdErr(if self.chance(0.5) {
                StdErr::ParseInt
            } else {
                StdErr::ParseFloat
            }),
            _ => self.error_ty().unwrap_or(Ty::Str),
        };
        Ty::res_of(ok, err)
    }

    /// A user type the block declared, when there is one.
    pub(super) fn user_ty(&mut self) -> Option<Ty> {
        if self.types.is_empty() {
            return None;
        }
        let index = self.rng.random_range(0..self.types.len());
        Some(self.types[index].ty())
    }

    /// A user enum that converts from a std parse error, the error side of a
    /// generated `Result`.
    pub(super) fn error_ty(&mut self) -> Option<Ty> {
        let errors: Vec<Ty> = self
            .types
            .iter()
            .filter(|def| def.shape.is_enum() && !def.shape.froms.is_empty())
            .map(UserDef::ty)
            .collect();
        if errors.is_empty() {
            return None;
        }
        Some(self.pick(&errors).clone())
    }

    pub(super) fn scalar_ty(&mut self) -> Ty {
        self.pick(SCALAR_TYPES).clone()
    }

    pub(super) fn int_width(&mut self) -> IntWidth {
        *self.pick(INT_WIDTHS)
    }

    pub(super) fn float_width(&mut self) -> FloatWidth {
        *self.pick(FLOAT_WIDTHS)
    }
}

/// Whether `<` and `>` compile between two values of the type.
pub(super) fn is_partial_ord(ty: &Ty) -> bool {
    match ty {
        Ty::Float(_) => true,
        Ty::Vec(inner) | Ty::Opt(inner) => is_partial_ord(inner),
        Ty::Tuple(items) => items.iter().all(is_partial_ord),
        Ty::Res(ok, err) => is_partial_ord(ok) && is_partial_ord(err),
        other => other.is_ord(),
    }
}
