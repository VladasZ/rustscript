//! Comparison, call and closure expression generation.

use rand::RngExt;

use crate::lang::block::{Param, ParamMode};
use crate::lang::expr::{BinOp, Expr, ReadMode, UnOp, unbare_deep};
use crate::lang::own::{BindKind, OwnState};
use crate::lang::synth::{Generator, MAX_EXPR_DEPTH, MOVE_CHANCE, is_partial_ord};
use crate::lang::ty::{FloatWidth, IntWidth, Ty};
use crate::lang::user::{MethodKind, UserShape};

impl Generator<'_> {
    pub(super) fn comparison(&mut self, depth: usize) -> Expr {
        let operand = if self.chance(0.6) {
            self.scalar_ty()
        } else {
            self.any_ty()
        };
        let ops: &[BinOp] = if is_partial_ord(&operand) {
            &[
                BinOp::Eq,
                BinOp::Ne,
                BinOp::Lt,
                BinOp::Le,
                BinOp::Gt,
                BinOp::Ge,
            ]
        } else {
            &[BinOp::Eq, BinOp::Ne]
        };
        let op = *self.pick(ops);
        // a comparison takes both sides by reference
        let (left, right) = self.borrowing(|inner| {
            let left = inner.expr(&operand, depth - 1);
            let right = inner.expr(&operand, depth - 1);
            (left, right)
        });
        Expr::Bin {
            op,
            left: Box::new(left),
            right: Box::new(right),
            ty: Ty::Bool,
        }
    }

    pub(super) fn cast(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let source = match want {
            Ty::Int(_) | Ty::Float(_) => match self.rng.random_range(0..10) {
                0..=5 => Ty::Int(self.int_width()),
                6 | 7 => Ty::Float(self.float_width()),
                8 if want.is_int() => Ty::Bool,
                _ if want.is_int() => Ty::Char,
                _ => Ty::Int(self.int_width()),
            },
            // only `u8` casts to `char`
            Ty::Char => Ty::Int(IntWidth::U8),
            _ => return None,
        };
        // `char as f64` doesn't exist
        if matches!(source, Ty::Char) && !want.is_int() {
            return None;
        }
        let value = self.expr(&source, depth - 1);
        Some(Expr::Cast {
            value: Box::new(value),
            to: want.clone(),
        })
    }

    pub(super) fn unary(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let op = match want {
            Ty::Int(width) if width.is_signed() => {
                if self.chance(0.5) {
                    UnOp::Neg
                } else {
                    UnOp::Not
                }
            }
            Ty::Int(_) | Ty::Bool => UnOp::Not,
            Ty::Float(_) => UnOp::Neg,
            _ => return None,
        };
        let value = self.expr(want, depth - 1);
        Some(Expr::Unary {
            op,
            value: Box::new(value),
            ty: want.clone(),
        })
    }

    pub(super) fn if_expr(&mut self, want: &Ty, depth: usize) -> Expr {
        let condition = self.expr(&Ty::Bool, depth - 1);
        self.begin_branches();
        let then_expr = self.branch(|inner| inner.expr(want, depth - 1));
        let else_expr = self.branch(|inner| inner.expr(want, depth - 1));
        self.end_branches();
        Expr::If {
            condition: Box::new(condition),
            then_expr: Box::new(then_expr),
            else_expr: Box::new(else_expr),
            ty: want.clone(),
        }
    }

    /// A bare literal or a const read.
    pub(super) fn bare_or_const(&mut self, want: &Ty) -> Option<Expr> {
        let consts: Vec<String> = self
            .scope
            .visible()
            .into_iter()
            .filter(|slot| matches!(slot.kind, BindKind::Const) && slot.ty == *want)
            .map(|slot| slot.name.clone())
            .collect();
        if !consts.is_empty() && self.chance(0.5) {
            return Some(Expr::ConstRef {
                name: self.pick(&consts).clone(),
                ty: want.clone(),
                opaque: false,
            });
        }
        if self.forbid_bare {
            return None;
        }
        match want {
            Ty::Int(IntWidth::I32) => Some(Expr::BareInt {
                value: self.int_value(IntWidth::I32),
                opaque: false,
            }),
            Ty::Float(FloatWidth::F64) => Some(Expr::BareFloat {
                token: self.bare_float_token(),
                opaque: false,
            }),
            _ => None,
        }
    }

    // accesses

    pub(super) fn access(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let mut options = self.binding_accesses(want, depth);
        // a field read off a fresh struct value
        if options.is_empty() || self.chance(0.2) {
            let shapes: Vec<UserShape> = self
                .types
                .iter()
                .filter(|def| def.shape.fields().iter().any(|field| field.ty == *want))
                .map(|def| def.shape.clone())
                .collect();
            if let Some(shape) = shapes.first() {
                let shape = shape.clone();
                let index = shape.fields().iter().position(|field| field.ty == *want)?;
                let base = self.expr(&Ty::user(shape), depth - 1);
                // a field moved out of a temporary drops the rest of it at the semicolon, a
                // field moved out of a binding leaves it partially moved
                let mode = match &base {
                    Expr::Var { name, .. } => {
                        let mode = self.field_mode(&name.clone(), index, want);
                        if mode == ReadMode::Move {
                            self.scope.note_field_move(name, index);
                        }
                        mode
                    }
                    _ if !want.is_copy() && self.chance(0.5) => ReadMode::Move,
                    _ => ReadMode::Clone,
                };
                return Some(Expr::Field {
                    base: Box::new(base),
                    index,
                    ty: want.clone(),
                    mode,
                });
            }
        }
        if options.is_empty() {
            return None;
        }
        let chosen = options.swap_remove(self.rng.random_range(0..options.len()));
        if let Expr::Field {
            base,
            index,
            mode: ReadMode::Move,
            ..
        }
        | Expr::TupleField {
            base,
            index,
            mode: ReadMode::Move,
            ..
        } = &chosen
            && let Expr::Var { name, .. } = &**base
        {
            self.scope.note_field_move(name, *index);
        }
        Some(chosen)
    }

    /// Every field, tuple slot and vec index of a binding that gives `want`.
    fn binding_accesses(&mut self, want: &Ty, depth: usize) -> Vec<Expr> {
        let mut options: Vec<Expr> = Vec::new();
        // a partially moved binding still offers its other fields
        let locals: Vec<(String, Ty, OwnState)> = self
            .scope
            .visible()
            .into_iter()
            .filter(|slot| matches!(slot.kind, BindKind::Local) && slot.state != OwnState::Moved)
            .map(|slot| (slot.name.clone(), slot.ty.clone(), slot.state.clone()))
            .collect();
        for (name, ty, state) in &locals {
            let base = Expr::Var {
                name: name.clone(),
                ty: ty.clone(),
                mode: ReadMode::Clone,
            };
            match ty {
                Ty::User(shape) => {
                    for (index, field) in shape.fields().iter().enumerate() {
                        if field.ty == *want && self.scope.can_read_field(name, index) {
                            options.push(Expr::Field {
                                base: Box::new(base.clone()),
                                index,
                                ty: want.clone(),
                                mode: self.field_mode(name, index, want),
                            });
                        }
                    }
                }
                Ty::Tuple(items) => {
                    for (index, item) in items.iter().enumerate() {
                        if item == want && self.scope.can_read_field(name, index) {
                            options.push(Expr::TupleField {
                                base: Box::new(base.clone()),
                                index,
                                ty: want.clone(),
                                mode: self.field_mode(name, index, want),
                            });
                        }
                    }
                }
                Ty::Vec(elem) if **elem == *want && *state == OwnState::Owned => {
                    let index = if self.chance(0.7) {
                        Expr::IntLit {
                            width: IntWidth::USize,
                            value: i128::from(self.rng.random_range(0..=4u8)),
                            opaque: false,
                        }
                    } else {
                        self.expr(&Ty::USIZE, depth - 1)
                    };
                    options.push(Expr::Index {
                        base: Box::new(base.clone()),
                        index: Box::new(index),
                        ty: want.clone(),
                    });
                }
                _ => {}
            }
        }
        options
    }

    /// The draw for one field read, a move when the field can leave the binding.
    fn field_mode(&mut self, name: &str, index: usize, field: &Ty) -> ReadMode {
        if self.scope.can_move_field(name, index, field) && self.chance(MOVE_CHANCE) {
            ReadMode::Move
        } else {
            ReadMode::Clone
        }
    }

    // user types

    pub(super) fn user_expr(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let method = self.method_call(want, depth);
        if method.is_some() && self.chance(0.5) {
            return method;
        }
        let Ty::User(shape) = want else {
            if want.has_default() && self.chance(0.3) {
                return Some(Expr::DefaultOf(want.clone()));
            }
            return method;
        };
        let shape = shape.clone();
        match self.rng.random_range(0..6) {
            0 if shape.derives.default => Some(Expr::DefaultOf(want.clone())),
            1 if !shape.froms.is_empty() => {
                let src = self.pick(&shape.froms).clone();
                let value = unbare_deep(self.typed_only(|inner| inner.expr(&src, depth - 1)));
                Some(Expr::Into {
                    value: Box::new(value),
                    to: want.clone(),
                    bare: false,
                })
            }
            2 => self.assoc_call(&shape, depth),
            _ => Some(self.user_literal(&shape, depth - 1)),
        }
    }

    pub(super) fn user_literal(&mut self, shape: &UserShape, depth: usize) -> Expr {
        if shape.is_enum() {
            let variant = self.rng.random_range(0..shape.variants().len());
            let payload = shape.variants()[variant]
                .payload
                .iter()
                .map(|ty| self.expr(ty, depth))
                .collect();
            return Expr::EnumLit {
                shape: Box::new(shape.clone()),
                variant,
                payload,
            };
        }
        let fields = shape.fields();
        let update = shape.derives.default && fields.len() > 1 && self.chance(0.4);
        let written = if update {
            self.rng.random_range(0..fields.len())
        } else {
            fields.len()
        };
        let values = fields[..written]
            .iter()
            .map(|field| self.expr(&field.ty, depth))
            .collect();
        Expr::StructLit {
            shape: Box::new(shape.clone()),
            fields: values,
            update,
        }
    }

    pub(super) fn assoc_call(&mut self, shape: &UserShape, depth: usize) -> Option<Expr> {
        let assoc: Vec<_> = shape
            .methods
            .iter()
            .filter(|sig| sig.kind == MethodKind::Assoc)
            .cloned()
            .collect();
        if assoc.is_empty() {
            return None;
        }
        let sig = self.pick(&assoc).clone();
        let args = sig.args.iter().map(|ty| self.expr(ty, depth - 1)).collect();
        Some(Expr::Method {
            owner: Box::new(shape.clone()),
            name: sig.name,
            kind: MethodKind::Assoc,
            base: None,
            args,
            ty: Ty::user(shape.clone()),
        })
    }

    pub(super) fn method_call(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let mut options: Vec<(UserShape, crate::lang::user::MethodSig)> = Vec::new();
        for def in &self.types {
            let owner = def.ty();
            for sig in &def.shape.methods {
                if sig.kind == MethodKind::Method && sig.ret_ty(&owner) == *want {
                    options.push((def.shape.clone(), sig.clone()));
                }
            }
        }
        if options.is_empty() {
            return None;
        }
        let (shape, sig) = self.pick(&options).clone();
        let owner = Ty::user(shape.clone());
        let base = self.expr(&owner, depth - 1);
        // a binding receiver is borrowed while the arguments run
        let held = match &base {
            Expr::Var { name, .. } => Some(name.clone()),
            _ => None,
        };
        if let Some(name) = &held {
            self.scope.freeze(name);
        }
        let args = sig.args.iter().map(|ty| self.expr(ty, depth - 1)).collect();
        if held.is_some() {
            self.scope.unfreeze();
        }
        Some(Expr::Method {
            owner: Box::new(shape),
            name: sig.name,
            kind: MethodKind::Method,
            base: Some(Box::new(base)),
            args,
            ty: want.clone(),
        })
    }

    // named calls

    pub(super) fn call_named(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let mut options: Vec<Expr> = Vec::new();
        if let Some(expr) = self.closure_call(want, depth) {
            options.push(expr);
        }
        if let Some(expr) = self.helper_call(want, depth) {
            options.push(expr);
        }
        if *want == Ty::Str
            && let Some(expr) = self.trait_call(depth)
        {
            options.push(expr);
        }
        if self.chance(0.15) {
            let name = self.generic_pick_fn();
            let first = self.expr(want, depth - 1);
            let second = self.expr(want, depth - 1);
            let flag = self.expr(&Ty::Bool, depth - 1);
            options.push(Expr::FnCall {
                name,
                args: vec![first, second, flag],
                by_ref: Vec::new(),
                ty: want.clone(),
            });
        }
        if options.is_empty() {
            return None;
        }
        Some(self.pick(&options).clone())
    }

    pub(super) fn closure_call(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let closures: Vec<(String, Vec<Ty>)> = self
            .scope
            .visible()
            .into_iter()
            .filter(|slot| slot.state == OwnState::Owned)
            .filter_map(|slot| match &slot.kind {
                BindKind::Closure { params, ret } if ret == want => {
                    Some((slot.name.clone(), params.clone()))
                }
                _ => None,
            })
            .filter(|(name, _)| !self.called_closures.contains(name))
            .collect();
        if closures.is_empty() {
            return None;
        }
        let (name, params) = self.pick(&closures).clone();
        self.called_closures.push(name.clone());
        // a closure over its own return type can also go through the apply helper
        if params.len() == 1 && params[0] == *want && self.chance(0.3) {
            let helper = self.apply_fn(want);
            // the helper holds the closure by `&mut`, so the argument must not call it
            let arg = self.without_binding(&name, |inner| inner.expr(want, depth - 1));
            return Some(Expr::ApplyCall {
                helper,
                closure: name,
                arg: Box::new(arg),
                ty: want.clone(),
            });
        }
        // a `FnMut` borrow is exclusive, so the arguments must not reach the same closure
        let args = self.without_binding(&name, |inner| {
            params
                .iter()
                .map(|ty| inner.expr(ty, depth - 1))
                .collect::<Vec<_>>()
        });
        Some(Expr::ClosureCall {
            name,
            args,
            ty: want.clone(),
        })
    }

    pub(super) fn helper_call(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let plain: Vec<(String, Vec<Param>)> = self
            .fns
            .iter()
            .filter_map(|def| match &def.kind {
                crate::lang::block::FnKind::Plain { params, ret, .. } if ret == want => {
                    Some((def.name.clone(), params.clone()))
                }
                _ => None,
            })
            .collect();
        let (name, params) = if plain.is_empty() || self.chance(0.3) {
            // a new helper, so the wanted type gets a body out of sight
            if depth < MAX_EXPR_DEPTH {
                return None;
            }
            self.helper_fn(want)?
        } else {
            self.pick(&plain).clone()
        };
        Some(self.fn_call(name, &params, want, depth - 1))
    }

    pub(super) fn fn_call(
        &mut self,
        name: String,
        params: &[Param],
        ret: &Ty,
        depth: usize,
    ) -> Expr {
        // a `&T` parameter only borrows its argument
        let args = params
            .iter()
            .map(|param| match param.mode {
                ParamMode::Owned => self.expr(&param.ty, depth),
                ParamMode::Ref => self.borrowing(|inner| inner.expr(&param.ty, depth)),
            })
            .collect();
        Expr::FnCall {
            name,
            args,
            by_ref: params
                .iter()
                .map(|param| param.mode == ParamMode::Ref)
                .collect(),
            ty: ret.clone(),
        }
    }

    pub(super) fn trait_call(&mut self, depth: usize) -> Option<Expr> {
        let mut targets: Vec<Ty> = self.describes.clone();
        targets.extend(
            self.types
                .iter()
                .filter(|def| def.shape.describe)
                .map(crate::lang::user::UserDef::ty),
        );
        if targets.is_empty() {
            return None;
        }
        let ty = self.pick(&targets).clone();
        let base = unbare_deep(self.typed_only(|inner| inner.expr(&ty, depth - 1)));
        Some(Expr::TraitCall {
            base: Box::new(base),
        })
    }

    /// `value?` where the return type accepts it, through `From` if needed.
    pub(super) fn try_expr(&mut self, want: &Ty, depth: usize) -> Option<Expr> {
        let ret = self.fn_ret.clone()?;
        let inner = match &ret {
            Ty::Opt(_) => Ty::opt_of(want.clone()),
            Ty::Res(_, err) => {
                let mut sources = vec![(**err).clone()];
                if let Ty::User(shape) = &**err {
                    sources.extend(shape.froms.iter().cloned());
                }
                let source = self.pick(&sources).clone();
                Ty::res_of(want.clone(), source)
            }
            _ => return None,
        };
        let value = self.expr(&inner, depth - 1);
        Some(Expr::Try {
            value: Box::new(value),
            ty: want.clone(),
        })
    }
}
