//! Type directed generation. Every shape is chosen by type, so a catalog method appears at any
//! depth at once.

mod exprs;
mod exprs_calls;
mod matches;
mod pipes;
mod stmts;
mod stmts_loops;
mod users;

use rand::RngExt;
use rand::rngs::StdRng;

use crate::lang::block::{Block, ConstDef, FnDef};
use crate::lang::expr::Expr;
use crate::lang::ty::{
    FLOAT_WIDTHS, FloatWidth, INT_WIDTHS, IntWidth, MAX_TY_DEPTH, SCALAR_TYPES, StdErr, Ty,
};
use crate::lang::user::UserDef;

/// Enough for a call whose receiver is a call whose argument is an operator.
pub(super) const MAX_EXPR_DEPTH: usize = 3;

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
    /// baked into every item name so 2 blocks never collide
    pub(super) tag: usize,
    pub(super) types: Vec<UserDef>,
    pub(super) fns: Vec<FnDef>,
    pub(super) consts: Vec<ConstDef>,
    pub(super) describes: Vec<Ty>,
    /// lets `?` and an early `return` appear
    pub(super) fn_ret: Option<Ty>,
    /// lets `break` and `continue` appear
    pub(super) in_loop: bool,
    /// the labeled loops around the current body, a `break` or `continue` may name one
    pub(super) loop_labels: Vec<String>,
    /// a bare literal here would be an ambiguous `{integer}`
    pub(super) forbid_bare: bool,
    /// closures already called in the statement being built. A closure literal that names one
    /// holds it borrowed for as long as the literal lives, so a second mention in the same
    /// statement is 2 mutable borrows at once and rustc rejects it.
    pub(super) called_closures: Vec<String>,
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
            loop_labels: Vec::new(),
            forbid_bare: false,
            called_closures: Vec::new(),
        }
    }

    /// Wraps one statement, so the closures called inside it are forgotten again at its end.
    pub(super) fn statement<T>(&mut self, build: impl FnOnce(&mut Self) -> T) -> T {
        let saved = std::mem::take(&mut self.called_closures);
        let out = build(self);
        self.called_closures = saved;
        out
    }

    /// `?`, `break`, `continue` and `return` would apply to the closure, so none is offered.
    pub(super) fn closure_body<T>(&mut self, build: impl FnOnce(&mut Self) -> T) -> T {
        let saved_ret = self.fn_ret.take();
        let saved_loop = std::mem::replace(&mut self.in_loop, false);
        let out = build(self);
        self.fn_ret = saved_ret;
        self.in_loop = saved_loop;
        out
    }

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
            statements.push(self.statement(Self::binding_stmt));
        }
        let extras = self.rng.random_range(3..=7);
        for _ in 0..extras {
            statements.push(self.statement(Self::mutation));
        }
        // a few free standing expressions, so a value that was never stored still shows up
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
            let expr = self.statement(|inner| inner.expr(&ty, MAX_EXPR_DEPTH));
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

    // names and draws

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

    /// For the argument of a call that already borrows the binding.
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

    /// For a top level item body.
    pub(super) fn without_scope<T>(&mut self, build: impl FnOnce(&mut Self) -> T) -> T {
        let saved = std::mem::take(&mut self.scope);
        let saved_loop = std::mem::replace(&mut self.in_loop, false);
        let out = build(self);
        self.scope = saved;
        self.in_loop = saved_loop;
        out
    }

    // types

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

    pub(super) fn elem_ty(&mut self) -> Ty {
        match self.rng.random_range(0..10) {
            0 => self.tuple_ty(),
            1 => self.user_ty().unwrap_or_else(|| self.scalar_ty()),
            2 => Ty::vec_of(self.scalar_ty()),
            3 => Ty::opt_of(self.scalar_ty()),
            _ => self.scalar_ty(),
        }
    }

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

    /// See `is_map_val`.
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

    /// A user error enum when the block has one, else a parse error or `String`.
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

    pub(super) fn user_ty(&mut self) -> Option<Ty> {
        if self.types.is_empty() {
            return None;
        }
        let index = self.rng.random_range(0..self.types.len());
        Some(self.types[index].ty())
    }

    /// A user enum that converts from a std parse error.
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

pub(super) fn is_partial_ord(ty: &Ty) -> bool {
    match ty {
        Ty::Float(_) => true,
        Ty::Vec(inner) | Ty::Opt(inner) => is_partial_ord(inner),
        Ty::Tuple(items) => items.iter().all(is_partial_ord),
        Ty::Res(ok, err) => is_partial_ord(ok) && is_partial_ord(err),
        other => other.is_ord(),
    }
}
