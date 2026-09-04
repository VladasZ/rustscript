//! Statement generation.

use rand::RngExt;

use crate::lang::expr::{BinOp, Expr, ReadMode};
use crate::lang::own::{BindKind, OwnState, referenced};
use crate::lang::pipe::Site;
use crate::lang::stmt::{Ann, ClosureParam, ClosureSource, Stmt};
use crate::lang::synth::{Generator, MAX_EXPR_DEPTH};
use crate::lang::ty::Ty;

/// How a binding states its type.
enum Route {
    Plain,
    BareLet,
    Helper,
    CloneOf,
    MoveOf,
    Into,
}

impl Generator<'_> {
    pub(super) fn binding_stmt(&mut self) -> Stmt {
        if self.chance(0.12) {
            return self.closure_stmt();
        }
        if self.chance(0.1)
            && let Some(stmt) = self.take_binding()
        {
            return stmt;
        }
        let ty = self.any_ty();
        if let Ty::Tuple(items) = &ty
            && items.len() >= 2
            && self.chance(0.3)
        {
            let expr = self.expr(&ty, MAX_EXPR_DEPTH);
            let names: Vec<(String, Ty)> = items
                .iter()
                .map(|item| (self.fresh("v"), item.clone()))
                .collect();
            for (name, ty) in &names {
                self.push_let(name.clone(), ty.clone());
            }
            let ann = if expr.states_concrete_ty() && self.chance(0.5) {
                Ann::Inferred
            } else {
                Ann::Typed
            };
            return Stmt::LetTuple { names, expr, ann };
        }
        let name = self.binding_name();
        let (expr, ann) = match self.route(&ty) {
            Route::Plain => {
                let expr = self.expr(&ty, MAX_EXPR_DEPTH);
                let ann = self.ann_for(&expr);
                (expr, ann)
            }
            Route::CloneOf => {
                let expr = self
                    .clone_source(&ty)
                    .unwrap_or_else(|| self.expr(&ty, MAX_EXPR_DEPTH));
                let ann = self.ann_for(&expr);
                (expr, ann)
            }
            Route::MoveOf => {
                let expr = self
                    .move_source(&ty)
                    .unwrap_or_else(|| self.expr(&ty, MAX_EXPR_DEPTH));
                let ann = self.ann_for(&expr);
                (expr, ann)
            }
            // only the annotation states the type
            Route::BareLet => (
                self.pipe_collect(&ty, Site::Bare, MAX_EXPR_DEPTH)
                    .unwrap_or_else(|| self.expr(&ty, MAX_EXPR_DEPTH)),
                Ann::Typed,
            ),
            // the helper return type states it
            Route::Helper => {
                let expr = if let Some((fn_name, params)) = self.helper_fn(&ty) {
                    self.fn_call(fn_name, &params, &ty, MAX_EXPR_DEPTH - 1)
                } else {
                    self.expr(&ty, MAX_EXPR_DEPTH)
                };
                let ann = self.ann_for(&expr);
                (expr, ann)
            }
            // `let x: T = value.into();`, the annotation picks the `From`
            Route::Into => {
                let Ty::User(shape) = &ty else {
                    unreachable!("into route only on user types");
                };
                let src = self.pick(&shape.froms).clone();
                let value = self.expr(&src, MAX_EXPR_DEPTH - 1);
                (
                    Expr::Into {
                        value: Box::new(value),
                        to: ty.clone(),
                        bare: true,
                    },
                    Ann::Typed,
                )
            }
        };
        self.push_let(name.clone(), ty.clone());
        Stmt::Let {
            name,
            ty,
            expr,
            ann,
            mutable: false,
        }
    }

    /// A fresh name, or now and then the name of a live local, which the new binding shadows.
    /// The old one stays alive under it until the scope ends.
    fn binding_name(&mut self) -> String {
        let locals = self.live_locals();
        if !locals.is_empty() && self.chance(0.15) {
            return self.pick(&locals).0.clone();
        }
        self.fresh("v")
    }

    /// An unannotated `let` needs an initializer that pins its type, a bare literal leaves an
    /// `{integer}` no inherent method can use.
    pub(super) fn ann_for(&mut self, expr: &Expr) -> Ann {
        let states = expr.states_concrete_ty();
        if states && self.chance(0.4) {
            Ann::Inferred
        } else {
            Ann::Typed
        }
    }

    fn route(&mut self, ty: &Ty) -> Route {
        match ty {
            Ty::Vec(_) | Ty::Map(..) | Ty::Set(_) | Ty::Int(_) | Ty::Float(_) => {
                match self.rng.random_range(0..7) {
                    0 => Route::BareLet,
                    1 => Route::Helper,
                    2 => Route::CloneOf,
                    3 => Route::MoveOf,
                    _ => Route::Plain,
                }
            }
            Ty::User(shape) if !shape.froms.is_empty() && self.chance(0.3) => Route::Into,
            _ => match self.rng.random_range(0..10) {
                0 => Route::CloneOf,
                1 | 2 => Route::MoveOf,
                _ => Route::Plain,
            },
        }
    }

    /// For a clone initialized `let`. Later mutations must stay private to the binding they hit,
    /// this checks copy on write.
    pub(super) fn clone_source(&mut self, ty: &Ty) -> Option<Expr> {
        let candidates = self.locals_of(ty);
        if candidates.is_empty() {
            return None;
        }
        Some(Expr::Var {
            name: self.pick(&candidates).clone(),
            ty: ty.clone(),
            mode: ReadMode::Clone,
        })
    }

    /// `let y = x;`, the old binding is gone after it.
    fn move_source(&mut self, ty: &Ty) -> Option<Expr> {
        let candidates: Vec<String> = self
            .locals_of(ty)
            .into_iter()
            .filter(|name| self.scope.can_move(name))
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let name = self.pick(&candidates).clone();
        self.scope.note_move(&name);
        Some(Expr::Var {
            name,
            ty: ty.clone(),
            mode: ReadMode::Move,
        })
    }

    // closures

    fn closure_stmt(&mut self) -> Stmt {
        let name = self.fresh("diff_cl");
        if self.chance(0.25) {
            return self.factory_closure(name);
        }
        let params = self.closure_params();
        let locals: Vec<(String, Ty)> = params.iter().flat_map(ClosureParam::locals).collect();
        let capture_move = self.chance(0.5);
        let ints: Vec<String> = self
            .live_locals()
            .into_iter()
            .filter(|(_, ty)| matches!(ty, Ty::Int(_)))
            .map(|(name, _)| name)
            .collect();
        let mutates = !ints.is_empty() && self.chance(0.4);
        let (ret, body) = if mutates {
            let acc = self.pick(&ints).clone();
            let acc_ty = self
                .scope
                .slot(&acc)
                .map_or(Ty::I64, |slot| slot.ty.clone());
            let expr = self.capturing(capture_move, |inner| {
                inner.closure_body(|inner| {
                    inner.with_locals(&locals, |inner| inner.expr(&acc_ty, 1))
                })
            });
            let op = *self.pick(&[BinOp::Add, BinOp::Sub, BinOp::BitXor, BinOp::Mul]);
            let body = Expr::Block {
                stmts: vec![Stmt::Compound {
                    name: acc.clone(),
                    op,
                    expr,
                }],
                tail: Box::new(Expr::Var {
                    name: acc,
                    ty: acc_ty.clone(),
                    mode: ReadMode::Clone,
                }),
            };
            (acc_ty, body)
        } else if capture_move {
            // a `move` closure owns what it names, so the body sees the outer scope and every
            // non copy binding it reads leaves it
            let ret = self.scalar_ty();
            let body = self.capturing(true, |inner| {
                inner.closure_body(|inner| inner.with_locals(&locals, |inner| inner.expr(&ret, 2)))
            });
            (ret, body)
        } else {
            // a borrowing closure would hold every binding it names until its last call, so
            // it sees only its own parameters
            let ret = self.scalar_ty();
            let body = self.closure_body(|inner| {
                inner.without_scope(|inner| inner.with_locals(&locals, |inner| inner.expr(&ret, 2)))
            });
            (ret, body)
        };
        let param_tys: Vec<Ty> = params.iter().map(|param| param.ty().clone()).collect();
        // the closure still holds the counter, so the arguments must not read it
        let hidden: Vec<String> = match &body {
            Expr::Block { stmts, .. } => stmts.iter().flat_map(Stmt::declared_targets).collect(),
            _ => Vec::new(),
        };
        let mut lifted = Vec::new();
        for name in &hidden {
            if let Some(slot) = self.scope.hide(name) {
                lifted.push(slot);
            }
        }
        // a borrowing closure holds what it names until its last call, so no argument may take
        // one of those
        let borrowed: Vec<String> = if capture_move {
            Vec::new()
        } else {
            referenced(&body).into_iter().collect()
        };
        for name in &borrowed {
            self.scope.freeze(name);
        }
        let calls = self.closure_calls(&name, &param_tys, &ret);
        for _ in &borrowed {
            self.scope.unfreeze();
        }
        for slot in lifted.into_iter().rev() {
            self.scope.unhide(slot);
        }
        // A mutably borrowing closure is called right after its definition and never again.
        // `move` and pure closures stay callable.
        if capture_move || !mutates {
            self.scope.push(
                name.clone(),
                ret.clone(),
                BindKind::Closure {
                    params: param_tys,
                    ret: ret.clone(),
                },
            );
        }
        Stmt::LetClosure {
            name,
            source: ClosureSource::Literal {
                params,
                ret,
                body,
                capture_move,
                mutates,
            },
            calls,
        }
    }

    /// Builds a closure body. With `capture_move` the body sees the outer bindings that may
    /// leave here, every other non copy one is hidden, and the ones it names are moved after.
    fn capturing(&mut self, capture_move: bool, build: impl FnOnce(&mut Self) -> Expr) -> Expr {
        if !capture_move {
            return build(self);
        }
        let stuck: Vec<String> = self
            .scope
            .visible()
            .into_iter()
            .filter(|slot| {
                let stays = match slot.kind {
                    BindKind::Local => !slot.is_copy() || slot.state != OwnState::Owned,
                    BindKind::Closure { .. } => true,
                    BindKind::Const => false,
                };
                stays && !self.scope.can_move(&slot.name)
            })
            .map(|slot| slot.name.clone())
            .collect();
        let mut lifted = Vec::new();
        for name in &stuck {
            if let Some(slot) = self.scope.hide(name) {
                lifted.push(slot);
            }
        }
        let body = build(self);
        for slot in lifted.into_iter().rev() {
            self.scope.unhide(slot);
        }
        for name in referenced(&body) {
            if self.scope.can_move(&name) {
                self.scope.note_move(&name);
            }
        }
        body
    }

    /// Up to 2 parameters, a pair pattern among them now and then.
    fn closure_params(&mut self) -> Vec<ClosureParam> {
        let count = self.rng.random_range(0..=2);
        (0..count)
            .map(|_| {
                if self.chance(0.3) {
                    let ty = Ty::Tuple(vec![self.scalar_ty(), self.scalar_ty()]);
                    ClosureParam::Pair {
                        first: self.fresh("diff_pa"),
                        second: self.fresh("diff_pb"),
                        ty,
                    }
                } else {
                    let ty = if self.chance(0.15) {
                        Ty::Trace
                    } else {
                        self.scalar_ty()
                    };
                    ClosureParam::Plain {
                        name: self.fresh("diff_p"),
                        ty,
                    }
                }
            })
            .collect()
    }

    fn factory_closure(&mut self, name: String) -> Stmt {
        let ty = Ty::Int(self.int_width());
        let fn_name = self.factory_fn(&ty);
        let arg = self.expr(&ty, MAX_EXPR_DEPTH - 1);
        let calls = self.closure_calls(&name, std::slice::from_ref(&ty), &ty);
        self.scope.push(
            name.clone(),
            ty.clone(),
            BindKind::Closure {
                params: vec![ty.clone()],
                ret: ty.clone(),
            },
        );
        Stmt::LetClosure {
            name,
            source: ClosureSource::Factory { fn_name, arg, ty },
            calls,
        }
    }

    /// The calls printed right after a closure binding. A closure over its own return type also
    /// goes through the apply helper.
    fn closure_calls(&mut self, name: &str, params: &[Ty], ret: &Ty) -> Vec<Expr> {
        let count = self.rng.random_range(1..=3);
        let applies = params.len() == 1 && params[0] == *ret;
        (0..count)
            .map(|_| {
                self.statement(|inner| {
                    if applies && inner.chance(0.35) {
                        let helper = inner.apply_fn(ret);
                        let arg = inner.expr(ret, 1);
                        return Expr::ApplyCall {
                            helper,
                            closure: name.to_string(),
                            arg: Box::new(arg),
                            ty: ret.clone(),
                        };
                    }
                    let args = params.iter().map(|ty| inner.expr(ty, 1)).collect();
                    Expr::ClosureCall {
                        name: name.to_string(),
                        args,
                        ty: ret.clone(),
                    }
                })
            })
            .collect()
    }

    // mutations

    pub(super) fn mutation(&mut self) -> Stmt {
        match self.rng.random_range(0..16) {
            0 => self.assign_stmt(),
            1 => self.compound_stmt().unwrap_or_else(|| self.assign_stmt()),
            2 => self
                .collection_mutation()
                .unwrap_or_else(|| self.observation()),
            3 => self
                .accumulation_loop()
                .unwrap_or_else(|| self.observation()),
            4 => self.if_stmt(),
            5 => self.for_stmt(),
            6 => self.while_stmt(),
            7 => self.for_mut_stmt().unwrap_or_else(|| self.observation()),
            8 => self.call_mut_stmt().unwrap_or_else(|| self.observation()),
            9 if self.in_loop => self.break_or_continue(),
            10 if self.fn_ret.is_some() => self.return_stmt(),
            11 => self
                .assign_field_stmt()
                .unwrap_or_else(|| self.assign_stmt()),
            12 => self.swap_stmt().unwrap_or_else(|| self.observation()),
            13 => self.scope_stmt(),
            _ => self.observation(),
        }
    }

    pub(super) fn observation(&mut self) -> Stmt {
        let ty = self.any_ty();
        let expr = self.borrowing(|inner| inner.expr(&ty, MAX_EXPR_DEPTH));
        self.print_stmt(expr)
    }

    pub(super) fn pick_local(&mut self) -> Option<(String, Ty)> {
        let locals = self.live_locals();
        if locals.is_empty() {
            return None;
        }
        Some(self.pick(&locals).clone())
    }

    /// Writes a binding, which also brings a moved one back.
    pub(super) fn assign_stmt(&mut self) -> Stmt {
        let targets: Vec<(String, Ty)> = self
            .scope
            .visible()
            .into_iter()
            .filter(|slot| {
                matches!(slot.kind, BindKind::Local) && self.scope.can_assign(&slot.name)
            })
            .map(|slot| (slot.name.clone(), slot.ty.clone()))
            .collect();
        if targets.is_empty() {
            return self.observation();
        }
        let (name, ty) = self.pick(&targets).clone();
        let mut expr = self.expr(&ty, MAX_EXPR_DEPTH - 1);
        // `x = x` is a self assignment `rustc` warns about
        if let Expr::Var {
            name: read, mode, ..
        } = &mut expr
            && *read == name
        {
            *mode = ReadMode::Clone;
        }
        self.scope.revive(&name);
        Stmt::Assign { name, expr }
    }

    /// `name.field = expr;`, a moved out field is put back too.
    fn assign_field_stmt(&mut self) -> Option<Stmt> {
        let mut targets: Vec<(String, Ty, usize, Ty)> = Vec::new();
        for slot in self.scope.visible() {
            if !matches!(slot.kind, BindKind::Local) {
                continue;
            }
            let fields: Vec<Ty> = match &slot.ty {
                Ty::User(shape) => shape.fields().iter().map(|f| f.ty.clone()).collect(),
                Ty::Tuple(items) => items.clone(),
                _ => continue,
            };
            for (index, field) in fields.into_iter().enumerate() {
                if self.scope.can_assign_field(&slot.name, index) {
                    targets.push((slot.name.clone(), slot.ty.clone(), index, field));
                }
            }
        }
        if targets.is_empty() {
            return None;
        }
        let (name, base, index, field) = self.pick(&targets).clone();
        // a field write needs the whole binding in place, so the value can't take it
        let expr = self.holding(&name, |inner| inner.expr(&field, MAX_EXPR_DEPTH - 1));
        self.scope.revive_field(&name, index);
        Some(Stmt::AssignField {
            name,
            base,
            index,
            expr,
        })
    }

    /// `std::mem::swap` of 2 live bindings of one type.
    fn swap_stmt(&mut self) -> Option<Stmt> {
        let locals: Vec<(String, Ty)> = self
            .live_locals()
            .into_iter()
            .filter(|(name, _)| self.scope.can_mem(name))
            .collect();
        let mut pairs = Vec::new();
        for (index, (a, ty)) in locals.iter().enumerate() {
            for (b, other) in &locals[index + 1..] {
                if ty == other {
                    pairs.push((a.clone(), b.clone()));
                }
            }
        }
        if pairs.is_empty() {
            return None;
        }
        let (a, b) = self.pick(&pairs).clone();
        Some(Stmt::Swap { a, b })
    }

    /// A bare block, so its bindings drop at the closing brace.
    fn scope_stmt(&mut self) -> Stmt {
        let body = self.scoped(Self::nested_body);
        Stmt::Scope { body }
    }

    pub(super) fn compound_stmt(&mut self) -> Option<Stmt> {
        let (name, ty) = self.pick_local()?;
        let op = match &ty {
            Ty::Int(_) => *self.pick(&[
                BinOp::Add,
                BinOp::Sub,
                BinOp::Mul,
                BinOp::Div,
                BinOp::Rem,
                BinOp::BitAnd,
                BinOp::BitOr,
                BinOp::BitXor,
                BinOp::Shl,
                BinOp::Shr,
            ]),
            Ty::Float(_) => *self.pick(&[BinOp::Add, BinOp::Sub, BinOp::Mul, BinOp::Div]),
            Ty::Bool => *self.pick(&[BinOp::BitAnd, BinOp::BitOr, BinOp::BitXor]),
            Ty::Str => BinOp::Add,
            _ => return None,
        };
        let rhs_ty = if matches!(op, BinOp::Shl | BinOp::Shr) {
            Ty::U32
        } else {
            ty
        };
        // the target is borrowed for the write, so the right side can't take it
        let expr = self.holding(&name, |inner| inner.expr(&rhs_ty, MAX_EXPR_DEPTH - 1));
        Some(Stmt::Compound { name, op, expr })
    }
}
