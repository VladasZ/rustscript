//! Statement generation.

use std::collections::BTreeSet;

use rand::RngExt;

use crate::lang::expr::{BinOp, Expr};
use crate::lang::fmt::{Align, FmtSpec, FmtTrait};
use crate::lang::pipe::Site;
use crate::lang::stmt::{Ann, ClosureParam, ClosureSource, MutOp, PrintForm, Stmt};
use crate::lang::synth::{BindKind, Binding, Generator, MAX_EXPR_DEPTH};
use crate::lang::ty::Ty;

/// How a binding states its type.
enum Route {
    Plain,
    BareLet,
    Helper,
    CloneOf,
    Into,
}

impl Generator<'_> {
    pub(super) fn binding_stmt(&mut self) -> Stmt {
        if self.chance(0.12) {
            return self.closure_stmt();
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
                self.push_local(name.clone(), ty.clone());
            }
            let ann = if expr.states_concrete_ty() && self.chance(0.5) {
                Ann::Inferred
            } else {
                Ann::Typed
            };
            return Stmt::LetTuple { names, expr, ann };
        }
        let name = self.fresh("v");
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
        self.push_local(name.clone(), ty.clone());
        Stmt::Let {
            name,
            ty,
            expr,
            ann,
        }
    }

    /// An unannotated `let` needs an initializer that pins its type, a bare literal leaves an
    /// `{integer}` no inherent method can use.
    fn ann_for(&mut self, expr: &Expr) -> Ann {
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
                match self.rng.random_range(0..6) {
                    0 => Route::BareLet,
                    1 => Route::Helper,
                    2 => Route::CloneOf,
                    _ => Route::Plain,
                }
            }
            Ty::User(shape) if !shape.froms.is_empty() && self.chance(0.3) => Route::Into,
            _ => {
                if self.chance(0.15) {
                    Route::CloneOf
                } else {
                    Route::Plain
                }
            }
        }
    }

    /// For a clone initialized `let`. Later mutations must stay private to the binding they hit,
    /// this checks copy on write.
    fn clone_source(&mut self, ty: &Ty) -> Option<Expr> {
        let candidates = self.locals_of(ty);
        if candidates.is_empty() {
            return None;
        }
        Some(Expr::Var {
            name: self.pick(&candidates).clone(),
            ty: ty.clone(),
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
            .scope
            .iter()
            .filter(|binding| {
                matches!(binding.kind, BindKind::Local) && matches!(binding.ty, Ty::Int(_))
            })
            .map(|binding| binding.name.clone())
            .collect();
        let mutates = !ints.is_empty() && self.chance(0.4);
        let (ret, body) = if mutates {
            let acc = self.pick(&ints).clone();
            let acc_ty = self
                .scope
                .iter()
                .find(|binding| binding.name == acc)
                .map_or(Ty::I64, |binding| binding.ty.clone());
            let expr = self
                .closure_body(|inner| inner.with_locals(&locals, |inner| inner.expr(&acc_ty, 1)));
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
                }),
            };
            (acc_ty, body)
        } else {
            let ret = self.scalar_ty();
            let body = self.closure_body(|inner| {
                inner.without_scope(|inner| inner.with_locals(&locals, |inner| inner.expr(&ret, 2)))
            });
            (ret, body)
        };
        // a `move` closure owns every non `Copy` local it names, so those leave the scope
        if capture_move {
            let moved: Vec<String> = self
                .scope
                .iter()
                .filter(|binding| matches!(binding.kind, BindKind::Local) && !binding.ty.is_copy())
                .map(|binding| binding.name.clone())
                .filter(|name| body.uses_any(&BTreeSet::from([name.clone()])))
                .collect();
            self.scope.retain(|binding| !moved.contains(&binding.name));
        }
        let param_tys: Vec<Ty> = params.iter().map(|param| param.ty().clone()).collect();
        // the closure still holds the counter, so the arguments must not read it
        let hidden = match &body {
            Expr::Block { stmts, .. } => stmts.iter().flat_map(Stmt::declared_targets).collect(),
            _ => Vec::new(),
        };
        let removed: Vec<Binding> = hidden
            .iter()
            .filter_map(|name| {
                let index = self
                    .scope
                    .iter()
                    .position(|binding| binding.name == *name)?;
                Some(self.scope.remove(index))
            })
            .collect();
        let calls = self.closure_calls(&name, &param_tys, &ret);
        self.scope.extend(removed);
        // A mutably borrowing closure is called right after its definition and never again.
        // `move` and pure closures stay callable.
        if capture_move || !mutates {
            self.scope.push(Binding {
                name: name.clone(),
                ty: ret.clone(),
                kind: BindKind::Closure {
                    params: param_tys,
                    ret: ret.clone(),
                },
            });
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
                    ClosureParam::Plain {
                        name: self.fresh("diff_p"),
                        ty: self.scalar_ty(),
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
        self.scope.push(Binding {
            name: name.clone(),
            ty: ty.clone(),
            kind: BindKind::Closure {
                params: vec![ty.clone()],
                ret: ty.clone(),
            },
        });
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
                if applies && self.chance(0.35) {
                    let helper = self.apply_fn(ret);
                    let arg = self.expr(ret, 1);
                    return Expr::ApplyCall {
                        helper,
                        closure: name.to_string(),
                        arg: Box::new(arg),
                        ty: ret.clone(),
                    };
                }
                let args = params.iter().map(|ty| self.expr(ty, 1)).collect();
                Expr::ClosureCall {
                    name: name.to_string(),
                    args,
                    ty: ret.clone(),
                }
            })
            .collect()
    }

    // mutations

    pub(super) fn mutation(&mut self) -> Stmt {
        match self.rng.random_range(0..12) {
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
            _ => self.observation(),
        }
    }

    fn observation(&mut self) -> Stmt {
        let ty = self.any_ty();
        let expr = self.expr(&ty, MAX_EXPR_DEPTH);
        self.print_stmt(expr)
    }

    fn pick_local(&mut self) -> Option<(String, Ty)> {
        let locals: Vec<(String, Ty)> = self
            .scope
            .iter()
            .filter(|binding| matches!(binding.kind, BindKind::Local))
            .map(|binding| (binding.name.clone(), binding.ty.clone()))
            .collect();
        if locals.is_empty() {
            return None;
        }
        Some(self.pick(&locals).clone())
    }

    fn assign_stmt(&mut self) -> Stmt {
        let Some((name, ty)) = self.pick_local() else {
            return self.observation();
        };
        Stmt::Assign {
            name,
            expr: self.expr(&ty, MAX_EXPR_DEPTH - 1),
        }
    }

    fn compound_stmt(&mut self) -> Option<Stmt> {
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
        let expr = self.expr(&rhs_ty, MAX_EXPR_DEPTH - 1);
        Some(Stmt::Compound { name, op, expr })
    }

    fn if_stmt(&mut self) -> Stmt {
        let condition = self.expr(&Ty::Bool, 2);
        let then_body = self.nested_body();
        let else_body = self.nested_body();
        Stmt::If {
            condition,
            then_body,
            else_body,
        }
    }

    fn for_stmt(&mut self) -> Stmt {
        let count = self.rng.random_range(0..=3);
        let (body, label) = self.loop_body();
        // the counter is never read, a name would make every program warn
        Stmt::ForRange {
            var: "_".to_string(),
            count,
            body,
            label,
        }
    }

    fn while_stmt(&mut self) -> Stmt {
        let counter = self.fresh("diff_i");
        let limit = self.rng.random_range(0..=4);
        let (body, label) = self.loop_body();
        if self.chance(0.5) {
            Stmt::While {
                counter,
                limit,
                body,
                label,
            }
        } else {
            Stmt::Loop {
                counter,
                limit,
                body,
                label,
            }
        }
    }

    /// A labeled exit may name any loop around it, so a nested loop exits through its parent.
    fn break_or_continue(&mut self) -> Stmt {
        let condition = self.expr(&Ty::Bool, 2);
        let label = if !self.loop_labels.is_empty() && self.chance(0.5) {
            let labels = self.loop_labels.clone();
            Some(self.pick(&labels).clone())
        } else {
            None
        };
        if self.chance(0.5) {
            Stmt::Break { condition, label }
        } else {
            Stmt::Continue { condition, label }
        }
    }

    pub(super) fn return_stmt(&mut self) -> Stmt {
        let ret = self.fn_ret.clone().unwrap_or(Ty::Bool);
        let condition = self.expr(&Ty::Bool, 2);
        let value = self.expr(&ret, 2);
        Stmt::Return { condition, value }
    }

    /// A nested block declares nothing, so the reducer never has to reason about shadowing.
    pub(super) fn nested_body(&mut self) -> Vec<Stmt> {
        let count = self.rng.random_range(1..=2);
        let mut body = Vec::new();
        for _ in 0..count {
            let stmt = match self.rng.random_range(0..6) {
                0 => self.assign_stmt(),
                1 => self.compound_stmt().unwrap_or_else(|| self.observation()),
                2 => self
                    .collection_mutation()
                    .unwrap_or_else(|| self.observation()),
                3 if self.in_loop => self.break_or_continue(),
                4 if self.fn_ret.is_some() => self.return_stmt(),
                _ => self.observation(),
            };
            body.push(stmt);
        }
        body
    }

    /// The body and the loop's label. A label nobody names is dropped, it would only warn.
    fn loop_body(&mut self) -> (Vec<Stmt>, Option<String>) {
        let was = std::mem::replace(&mut self.in_loop, true);
        let label = self.chance(0.5).then(|| self.fresh("diff_l"));
        if let Some(label) = &label {
            self.loop_labels.push(label.clone());
        }
        let body = self.nested_body();
        if label.is_some() {
            self.loop_labels.pop();
        }
        self.in_loop = was;
        let label = label.filter(|l| body.iter().any(|stmt| stmt.targets_label(l)));
        (body, label)
    }

    /// `name` is hidden because an `entry()` chain holds the map while its arguments evaluate.
    fn op_without(&mut self, name: &str, build: impl FnOnce(&mut Self) -> MutOp) -> MutOp {
        let index = self.scope.iter().position(|binding| binding.name == name);
        let removed = index.map(|found| self.scope.remove(found));
        let op = build(self);
        if let Some(binding) = removed {
            self.scope.push(binding);
        }
        op
    }

    fn collection_mutation(&mut self) -> Option<Stmt> {
        let (name, ty) = self.pick_mutable()?;
        let op = match &ty {
            Ty::Vec(elem) => {
                let elem = (**elem).clone();
                match self.rng.random_range(0..11) {
                    0 => MutOp::VecPush(self.expr(&elem, 1)),
                    1 if elem.is_ord() => MutOp::VecSort,
                    2 => match &elem {
                        Ty::Vec(inner) => MutOp::VecRowPush {
                            index: self.rng.random_range(0..=2),
                            value: self.expr(inner, 1),
                        },
                        _ => MutOp::VecDedup,
                    },
                    3 => MutOp::VecSetIndex {
                        index: self.rng.random_range(0..=6),
                        value: self.expr(&elem, 1),
                    },
                    4 => MutOp::VecReverse,
                    5 => MutOp::VecPop,
                    6 => MutOp::VecTruncate(self.rng.random_range(0..=3)),
                    7 => MutOp::VecSwap(self.rng.random_range(0..=4), self.rng.random_range(0..=4)),
                    8 => {
                        let bind = self.fresh("diff_r");
                        let locals = [(bind.clone(), elem.clone())];
                        // `retain` holds the vec, so the predicate must not read it
                        let pred = self.without_binding(&name, |inner| {
                            inner.closure_body(|inner| {
                                inner.with_locals(&locals, |inner| inner.expr(&Ty::Bool, 1))
                            })
                        });
                        MutOp::VecRetain { bind, pred }
                    }
                    9 => MutOp::VecClear,
                    _ => MutOp::VecExtend(self.expr(&Ty::vec_of(elem), 1)),
                }
            }
            Ty::Str => match self.rng.random_range(0..3) {
                0 => MutOp::StrPush(self.expr(&Ty::Char, 1)),
                1 => MutOp::StrClear,
                _ => MutOp::StrPushStr(self.expr(&Ty::Str, 1)),
            },
            Ty::Opt(elem) => {
                if self.chance(0.5) {
                    MutOp::OptTake
                } else {
                    MutOp::OptReplace(self.expr(elem, 1))
                }
            }
            Ty::Map(key, value) => {
                let key_ty = (**key).clone();
                let val_ty = (**value).clone();
                match (&val_ty, self.rng.random_range(0..4)) {
                    (Ty::Int(_), 0) => self.op_without(&name, |inner| MutOp::MapEntryAdd {
                        key: inner.expr(&key_ty, 1),
                        default: inner.expr(&val_ty, 1),
                        add: inner.expr(&val_ty, 1),
                    }),
                    (Ty::Vec(elem), 0) => {
                        let elem = (**elem).clone();
                        self.op_without(&name, |inner| MutOp::MapEntryPush {
                            key: inner.expr(&key_ty, 1),
                            value: inner.expr(&elem, 1),
                        })
                    }
                    (_, 1) => MutOp::MapRemove {
                        key: self.expr(&key_ty, 1),
                    },
                    _ => MutOp::MapInsert {
                        key: self.expr(&key_ty, 1),
                        value: self.expr(&val_ty, 1),
                    },
                }
            }
            Ty::Set(elem) => {
                let elem = (**elem).clone();
                if self.chance(0.7) {
                    MutOp::SetInsert(self.expr(&elem, 1))
                } else {
                    MutOp::SetRemove(self.expr(&elem, 1))
                }
            }
            _ => return None,
        };
        Some(Stmt::Mutate { name, op })
    }

    /// `for item in vec { accumulate into collection }`. The source is a vec, so order is defined.
    fn accumulation_loop(&mut self) -> Option<Stmt> {
        let (target, ty) = self.pick_collection()?;
        let var = self.fresh("diff_item");
        let (source_elem, op) = match &ty {
            Ty::Vec(elem) => {
                let elem = (**elem).clone();
                (
                    elem.clone(),
                    MutOp::VecPush(Expr::Var {
                        name: var.clone(),
                        ty: elem,
                    }),
                )
            }
            Ty::Set(elem) => {
                let elem = (**elem).clone();
                (
                    elem.clone(),
                    MutOp::SetInsert(Expr::Var {
                        name: var.clone(),
                        ty: elem,
                    }),
                )
            }
            Ty::Map(key, value) => {
                let key_ty = (**key).clone();
                let val_ty = (**value).clone();
                let key_expr = Expr::Var {
                    name: var.clone(),
                    ty: key_ty.clone(),
                };
                let op = match &val_ty {
                    Ty::Int(_) => self.op_without(&target, |inner| MutOp::MapEntryAdd {
                        key: key_expr,
                        default: inner.expr(&val_ty, 1),
                        add: inner.expr(&val_ty, 1),
                    }),
                    Ty::Vec(elem) => {
                        let elem = (**elem).clone();
                        self.op_without(&target, |inner| MutOp::MapEntryPush {
                            key: key_expr,
                            value: inner.expr(&elem, 1),
                        })
                    }
                    _ => MutOp::MapInsert {
                        key: key_expr,
                        value: self.expr(&val_ty, 1),
                    },
                };
                (key_ty, op)
            }
            _ => return None,
        };
        let source = self.expr(&Ty::vec_of(source_elem), 2);
        Some(Stmt::ForAccum {
            var,
            source,
            target,
            op,
        })
    }

    /// `for r in vec.iter_mut() { *r = expr(r) }` on a vec binding.
    fn for_mut_stmt(&mut self) -> Option<Stmt> {
        let vecs: Vec<(String, Ty)> = self
            .scope
            .iter()
            .filter(|binding| matches!(binding.kind, BindKind::Local))
            .filter_map(|binding| match &binding.ty {
                Ty::Vec(elem) => Some((binding.name.clone(), (**elem).clone())),
                _ => None,
            })
            .collect();
        if vecs.is_empty() {
            return None;
        }
        let (name, elem) = self.pick(&vecs).clone();
        let var = self.fresh("diff_e");
        // the vec is borrowed for the loop, so its name is hidden
        let index = self.scope.iter().position(|binding| binding.name == name);
        let removed = index.map(|found| self.scope.remove(found));
        let locals = [(var.clone(), elem.clone())];
        let expr = self.with_locals(&locals, |inner| inner.expr(&elem, 2));
        if let Some(binding) = removed {
            self.scope.push(binding);
        }
        Some(Stmt::ForMut {
            name,
            var,
            elem,
            expr,
        })
    }

    /// `helper(&mut binding, args)`
    fn call_mut_stmt(&mut self) -> Option<Stmt> {
        let (name, ty) = self.pick_local()?;
        let (fn_name, params) = self.writer_fn(&ty);
        // the target is borrowed for the call, so the arguments can't read it
        let index = self.scope.iter().position(|binding| binding.name == name);
        let removed = index.map(|found| self.scope.remove(found));
        let args = params.iter().map(|param| self.expr(param, 1)).collect();
        if let Some(binding) = removed {
            self.scope.push(binding);
        }
        Some(Stmt::CallMut {
            name,
            fn_name,
            args,
        })
    }

    fn pick_mutable(&mut self) -> Option<(String, Ty)> {
        let matching: Vec<(String, Ty)> = self
            .scope
            .iter()
            .filter(|binding| {
                matches!(binding.kind, BindKind::Local)
                    && matches!(
                        binding.ty,
                        Ty::Vec(_) | Ty::Map(..) | Ty::Set(_) | Ty::Str | Ty::Opt(_)
                    )
            })
            .map(|binding| (binding.name.clone(), binding.ty.clone()))
            .collect();
        if matching.is_empty() {
            return None;
        }
        Some(self.pick(&matching).clone())
    }

    fn pick_collection(&mut self) -> Option<(String, Ty)> {
        let matching: Vec<(String, Ty)> = self
            .scope
            .iter()
            .filter(|binding| {
                matches!(binding.kind, BindKind::Local)
                    && matches!(binding.ty, Ty::Vec(_) | Ty::Map(..) | Ty::Set(_))
            })
            .map(|binding| (binding.name.clone(), binding.ty.clone()))
            .collect();
        if matching.is_empty() {
            return None;
        }
        Some(self.pick(&matching).clone())
    }

    // observations

    pub(super) fn print_stmt(&mut self, expr: Expr) -> Stmt {
        let label = self.next_label();
        let observed = match expr.ty() {
            Ty::Map(key, value) => Ty::vec_of(Ty::Tuple(vec![*key, *value])),
            Ty::Set(elem) => Ty::vec_of(*elem),
            other => other,
        };
        let spec = self.fmt_spec(&observed);
        let form = match self.rng.random_range(0..10) {
            0 => PrintForm::Indexed,
            1 => PrintForm::Twice,
            2 => PrintForm::WidthArg(self.rng.random_range(0..=12)),
            3 => PrintForm::NamedWidth(self.rng.random_range(0..=12)),
            _ => PrintForm::Plain,
        };
        Stmt::Print {
            label,
            expr,
            spec,
            form,
        }
    }

    /// Plain `{:?}` a third of the time so observations stay readable.
    pub(super) fn fmt_spec(&mut self, ty: &Ty) -> FmtSpec {
        if self.chance(0.35) {
            return FmtSpec::DEBUG;
        }
        for _ in 0..6 {
            let fmt = *self.pick(&[
                FmtTrait::Display,
                FmtTrait::Display,
                FmtTrait::Debug,
                FmtTrait::Debug,
                FmtTrait::LowerHex,
                FmtTrait::UpperHex,
                FmtTrait::Octal,
                FmtTrait::Binary,
                FmtTrait::LowerExp,
                FmtTrait::UpperExp,
            ]);
            let mut spec = FmtSpec::plain(fmt);
            if self.chance(0.5) {
                spec.width = Some(self.rng.random_range(0..=12));
            }
            if self.chance(0.45) {
                spec.align = Some(*self.pick(&[Align::Left, Align::Right, Align::Center]));
                if self.chance(0.6) {
                    spec.fill = Some(*self.pick(&['*', '-', '=', '_', '~', '0']));
                }
            }
            spec.plus = self.chance(0.2);
            spec.zero = self.chance(0.2);
            spec.alternate = self.chance(0.25);
            if self.chance(0.3) {
                spec.precision = Some(self.rng.random_range(0..=4));
            }
            if spec.applies_to(ty) {
                return spec;
            }
        }
        FmtSpec::DEBUG
    }
}
