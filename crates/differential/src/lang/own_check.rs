//! Replays a finished block against the ownership rules of `own`, see `check_block`.

use std::collections::BTreeSet;

use crate::lang::block::{Block, FnKind};
use crate::lang::expr::{Expr, MemKind, ReadMode};
use crate::lang::own::{BindKind, Scope, referenced};
use crate::lang::pipe::{Bind, Item, Pipe, Source, Stage, Term};
use crate::lang::stmt::{ClosureSource, MutOp, Stmt};
use crate::lang::ty::Ty;
use crate::lang::user::MethodKind;

/// Whether every read in the block resolves to a binding it may read, or the first one that
/// does not. Every item body is checked with its parameters in scope.
pub fn check_block(block: &Block) -> Result<(), String> {
    let mut checker = Checker::default();
    checker.stmts(&block.statements);
    for def in &block.fns {
        checker.scope = Scope::default();
        match &def.kind {
            FnKind::Plain { params, body, .. } => {
                for param in params {
                    checker.push_local(&param.name, &param.ty);
                }
                checker.expr(body);
            }
            FnKind::Writer {
                target,
                params,
                value,
            } => {
                for param in params {
                    checker.push_local(&param.name, &param.ty);
                }
                checker.push_local("diff_cur", target);
                checker.expr(value);
            }
            FnKind::GenericPick | FnKind::Apply { .. } | FnKind::Factory { .. } => {}
        }
    }
    for def in &block.types {
        for method in &def.methods {
            checker.scope = Scope::default();
            if method.sig.kind == MethodKind::Method {
                checker.scope.push_borrowed("self".to_string(), def.ty());
                for field in def.shape.fields() {
                    checker
                        .scope
                        .push_borrowed(format!("self.{}", field.name), field.ty.clone());
                }
            }
            for (name, ty) in method.params.iter().zip(&method.sig.args) {
                checker.push_local(name, ty);
            }
            checker.expr(&method.body);
        }
    }
    match checker.fault {
        Some(fault) => Err(fault),
        None => Ok(()),
    }
}

#[derive(Default)]
struct Checker {
    scope: Scope,
    /// the first rule broken
    fault: Option<String>,
}

impl Checker {
    fn push_local(&mut self, name: &str, ty: &Ty) {
        self.scope
            .push(name.to_string(), ty.clone(), BindKind::Local);
    }

    fn push_let(&mut self, name: &str, ty: &Ty) {
        self.scope.push_let(name.to_string(), ty.clone());
    }

    fn require(&mut self, condition: bool, what: impl FnOnce() -> String) {
        if !condition && self.fault.is_none() {
            self.fault = Some(what());
        }
    }

    fn stmts(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            self.stmt(stmt);
        }
    }

    /// A body of its own scope, its bindings gone at the end.
    fn body(&mut self, stmts: &[Stmt]) {
        let mark = self.scope.len();
        self.stmts(stmts);
        self.scope.truncate(mark);
    }

    /// A loop body. Nothing it revives counts after it, the loop may run zero times.
    fn loop_body(&mut self, stmts: &[Stmt]) {
        let before = self.scope.snapshot();
        self.scope.enter_loop();
        self.body(stmts);
        self.scope.leave_loop();
        self.scope.restore(&before);
    }

    fn branches(&mut self, count: usize, mut build: impl FnMut(&mut Self, usize)) {
        let before = self.scope.snapshot();
        let mut ends = Vec::with_capacity(count);
        for index in 0..count {
            self.scope.restore(&before);
            let mark = self.scope.len();
            build(self, index);
            self.scope.truncate(mark);
            ends.push(self.scope.snapshot());
        }
        self.scope.merge(&before, &ends);
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { .. }
            | Stmt::LetTuple { .. }
            | Stmt::LetClosure { .. }
            | Stmt::Assign { .. }
            | Stmt::AssignField { .. }
            | Stmt::Compound { .. } => self.binding_stmt(stmt),
            Stmt::Print { expr, .. } => self.expr(expr),
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                self.expr(condition);
                self.branches(2, |inner, index| {
                    inner.stmts(if index == 0 { then_body } else { else_body });
                });
            }
            Stmt::ForRange { body, .. } | Stmt::While { body, .. } | Stmt::Loop { body, .. } => {
                self.loop_body(body);
            }
            Stmt::Break { condition, .. } | Stmt::Continue { condition, .. } => {
                self.expr(condition);
            }
            // the value runs on the leaving path only, so what it moves is still here after
            Stmt::Return { condition, value } => {
                self.expr(condition);
                let before = self.scope.snapshot();
                self.expr(value);
                self.scope.restore(&before);
            }
            Stmt::Mutate { name, op } => {
                self.require(self.scope.can_read(name), || format!("mutate `{name}`"));
                self.scope.freeze(name);
                self.mut_op(op);
                self.scope.unfreeze();
            }
            Stmt::ForAccum {
                var,
                source,
                target,
                op,
            } => {
                self.require(self.scope.can_read(target), || {
                    format!("accumulate into `{target}`")
                });
                self.scope.freeze(target);
                self.expr(source);
                let elem = match source.ty() {
                    Ty::Vec(elem) => *elem,
                    other => other,
                };
                self.loop_with(var, &elem, |inner| inner.mut_op(op));
                self.scope.unfreeze();
            }
            Stmt::ForMut {
                name,
                var,
                elem,
                expr,
            } => {
                self.require(self.scope.can_read(name), || {
                    format!("iter_mut over `{name}`")
                });
                let hidden = self.scope.hide(name);
                self.loop_with(var, elem, |inner| inner.expr(expr));
                if let Some(hidden) = hidden {
                    self.scope.unhide(hidden);
                }
            }
            Stmt::CallMut { name, args, .. } => {
                self.require(self.scope.can_read(name), || format!("&mut of `{name}`"));
                let hidden = self.scope.hide(name);
                for arg in args {
                    self.expr(arg);
                }
                if let Some(hidden) = hidden {
                    self.scope.unhide(hidden);
                }
            }
            Stmt::Swap { a, b } => {
                self.require(
                    a != b && self.scope.can_mem(a) && self.scope.can_mem(b),
                    || format!("swap of `{a}` and `{b}`"),
                );
            }
            Stmt::Scope { body } => self.body(body),
        }
    }

    /// The statements that declare or write a binding.
    fn binding_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { name, ty, expr, .. } => {
                self.expr(expr);
                self.push_let(name, ty);
            }
            Stmt::LetTuple { names, expr, .. } => {
                self.expr(expr);
                for (name, ty) in names {
                    self.push_let(name, ty);
                }
            }
            Stmt::LetClosure {
                name,
                source,
                calls,
            } => {
                self.closure(name, source);
                for call in calls {
                    self.expr(call);
                }
            }
            Stmt::Assign { name, expr } => {
                self.expr(expr);
                self.require(self.scope.can_assign(name), || {
                    format!("assign to `{name}`")
                });
                self.scope.revive(name);
            }
            Stmt::AssignField {
                name, index, expr, ..
            } => {
                self.expr(expr);
                self.require(self.scope.can_assign_field(name, *index), || {
                    format!("assign to field {index} of `{name}`")
                });
                self.scope.revive_field(name, *index);
            }
            Stmt::Compound { name, expr, .. } => {
                self.scope.freeze(name);
                self.expr(expr);
                self.scope.unfreeze();
                self.require(self.scope.can_read(name), || {
                    format!("compound on `{name}`")
                });
            }
            _ => unreachable!("binding_stmt handles the binding statements only"),
        }
    }

    /// A loop over items bound to `var`, see `loop_body`.
    fn loop_with(&mut self, var: &str, elem: &Ty, build: impl FnOnce(&mut Self)) {
        let before = self.scope.snapshot();
        self.scope.enter_loop();
        let mark = self.scope.len();
        self.push_local(var, elem);
        build(self);
        self.scope.truncate(mark);
        self.scope.leave_loop();
        self.scope.restore(&before);
    }

    fn mut_op(&mut self, op: &MutOp) {
        if let MutOp::VecRetain { bind, pred } = op {
            self.scope.enter_closure();
            let mark = self.scope.len();
            self.push_local(bind, &pred.ty());
            self.expr(pred);
            self.scope.truncate(mark);
            self.scope.leave_closure();
            return;
        }
        for expr in op.exprs() {
            self.expr(expr);
        }
    }

    fn closure(&mut self, name: &str, source: &ClosureSource) {
        match source {
            ClosureSource::Literal {
                params,
                ret,
                body,
                capture_move,
                ..
            } => {
                let captured: Vec<(String, Ty)> = if *capture_move {
                    referenced(body)
                        .into_iter()
                        .filter_map(|used| {
                            let slot = self.scope.slot(&used)?;
                            (!slot.is_copy()).then(|| (used.clone(), slot.ty.clone()))
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                for (used, _) in &captured {
                    self.require(self.scope.can_move(used), || {
                        format!("move capture of `{used}` by `{name}`")
                    });
                }
                self.scope.enter_closure();
                let mark = self.scope.len();
                // the closure owns its captures, so inside they are fresh locals
                for (used, ty) in &captured {
                    self.push_local(used, ty);
                }
                for param in params {
                    for (local, ty) in param.locals() {
                        self.push_local(&local, &ty);
                    }
                }
                self.expr(body);
                self.scope.truncate(mark);
                self.scope.leave_closure();
                for (used, _) in &captured {
                    self.scope.note_move(used);
                }
                let params: Vec<Ty> = params.iter().map(|param| param.ty().clone()).collect();
                self.scope.push(
                    name.to_string(),
                    ret.clone(),
                    BindKind::Closure {
                        params,
                        ret: ret.clone(),
                    },
                );
            }
            ClosureSource::Factory { arg, ty, .. } => {
                self.expr(arg);
                self.scope.push(
                    name.to_string(),
                    ty.clone(),
                    BindKind::Closure {
                        params: vec![ty.clone()],
                        ret: ty.clone(),
                    },
                );
            }
        }
    }

    fn read_var(&mut self, name: &str, ty: &Ty, mode: ReadMode) {
        if ty.is_copy() || mode == ReadMode::Clone {
            self.require(self.scope.can_read(name), || format!("read of `{name}`"));
            return;
        }
        self.require(self.scope.can_move(name), || format!("move of `{name}`"));
        self.scope.note_move(name);
    }

    fn read_field(&mut self, name: &str, index: usize, ty: &Ty, mode: ReadMode) {
        if ty.is_copy() || mode == ReadMode::Clone {
            self.require(self.scope.can_read_field(name, index), || {
                format!("read of field {index} of `{name}`")
            });
            return;
        }
        self.require(self.scope.can_move_field(name, index, ty), || {
            format!("move of field {index} of `{name}`")
        });
        self.scope.note_field_move(name, index);
    }

    /// A receiver that is a binding is used in place, never moved.
    fn place(&mut self, base: &Expr) {
        match base {
            Expr::Var { name, .. } => {
                self.require(self.scope.can_read(name), || {
                    format!("place use of `{name}`")
                });
            }
            other => self.expr(other),
        }
    }

    fn expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Var { name, ty, mode } => self.read_var(name, ty, *mode),
            Expr::Field {
                base,
                index,
                ty,
                mode,
            }
            | Expr::TupleField {
                base,
                index,
                ty,
                mode,
            } => match &**base {
                Expr::Var { name, .. } => self.read_field(name, *index, ty, *mode),
                other => self.expr(other),
            },
            Expr::Index { base, index, .. } => {
                self.place(base);
                self.expr(index);
            }
            Expr::Method { base, args, .. } => self.method(base.as_deref(), args),
            Expr::TraitCall { base } => self.place(base),
            Expr::Mem { name, kind, .. } => {
                self.require(self.scope.can_mem(name), || format!("mem op on `{name}`"));
                if let MemKind::Replace(value) = kind {
                    let hidden = self.scope.hide(name);
                    self.expr(value);
                    if let Some(hidden) = hidden {
                        self.scope.unhide(hidden);
                    }
                }
            }
            Expr::VecTake { name, .. } => {
                self.require(self.scope.can_mem(name), || format!("take out of `{name}`"));
            }
            Expr::ClosureCall { name, args, .. } => {
                self.require(self.scope.slot(name).is_some(), || {
                    format!("call of `{name}`")
                });
                for arg in args {
                    self.expr(arg);
                }
            }
            Expr::ApplyCall { closure, arg, .. } => {
                self.require(self.scope.slot(closure).is_some(), || {
                    format!("apply of `{closure}`")
                });
                self.expr(arg);
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                self.expr(scrutinee);
                self.branches(arms.len(), |inner, index| {
                    let arm = &arms[index];
                    let mut binds = Vec::new();
                    arm.pat.bindings(&mut binds);
                    for (name, ty) in &binds {
                        inner.push_local(name, ty);
                    }
                    if let Some(guard) = &arm.guard {
                        inner.expr(guard);
                    }
                    inner.expr(&arm.body);
                });
            }
            Expr::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => {
                self.expr(condition);
                self.branches(2, |inner, index| {
                    inner.expr(if index == 0 { then_expr } else { else_expr });
                });
            }
            Expr::Block { stmts, tail } => {
                let mark = self.scope.len();
                self.stmts(stmts);
                self.expr(tail);
                self.scope.truncate(mark);
            }
            Expr::Pipe(pipe) => self.pipe(pipe),
            _ => {
                for child in expr.children() {
                    self.expr(child);
                }
            }
        }
    }

    /// A binding receiver is borrowed while the arguments run.
    fn method(&mut self, base: Option<&Expr>, args: &[Expr]) {
        let held = match base {
            Some(Expr::Var { name, .. }) => {
                self.require(self.scope.can_read(name), || format!("method on `{name}`"));
                self.scope.freeze(name);
                true
            }
            Some(other) => {
                self.expr(other);
                false
            }
            None => false,
        };
        for arg in args {
            self.expr(arg);
        }
        if held {
            self.scope.unfreeze();
        }
    }

    fn pipe(&mut self, pipe: &Pipe) {
        if let Source::Coll { expr, .. } = &pipe.source {
            self.expr(expr);
        }
        let mut item = pipe.source.item();
        for stage in &pipe.stages {
            match stage {
                Stage::Map { bind, body, .. } | Stage::PairWith { bind, body } => {
                    self.pipe_body(bind, &item, body);
                }
                Stage::Filter { bind, pred, .. } => self.pipe_body(bind, &item, pred),
                _ => {}
            }
            item = stage.out(&item);
        }
        match &pipe.term {
            Term::Any { bind, pred } | Term::All { bind, pred } | Term::Position { bind, pred } => {
                self.pipe_body(bind, &item, pred);
            }
            Term::Fold {
                acc,
                bind,
                init,
                body,
            } => {
                // the stage closures hold what they name while the init runs
                let held: Vec<String> = pipe
                    .stages
                    .iter()
                    .flat_map(|stage| match stage {
                        Stage::Map { body, .. } | Stage::PairWith { body, .. } => referenced(body),
                        Stage::Filter { pred, .. } => referenced(pred),
                        _ => BTreeSet::new(),
                    })
                    .collect();
                for name in &held {
                    self.scope.freeze(name);
                }
                self.expr(init);
                for _ in &held {
                    self.scope.unfreeze();
                }
                let acc_item = match init.ty() {
                    Ty::Tuple(parts) if parts.len() == 2 => {
                        Item::Pair(parts[0].clone(), parts[1].clone())
                    }
                    other => Item::Scalar(other),
                };
                self.scope.enter_closure();
                let mark = self.scope.len();
                self.push_bind(acc, &acc_item);
                self.push_bind(bind, &item);
                self.expr(body);
                self.scope.truncate(mark);
                self.scope.leave_closure();
            }
            _ => {}
        }
    }

    fn pipe_body(&mut self, bind: &Bind, item: &Item, body: &Expr) {
        self.scope.enter_closure();
        let mark = self.scope.len();
        self.push_bind(bind, item);
        self.expr(body);
        self.scope.truncate(mark);
        self.scope.leave_closure();
    }

    fn push_bind(&mut self, bind: &Bind, item: &Item) {
        match (bind, item) {
            (Bind::One(name), Item::Scalar(ty)) => self.push_local(name, ty),
            (Bind::One(name), Item::Pair(key, value)) => {
                self.push_local(name, &Ty::Tuple(vec![key.clone(), value.clone()]));
            }
            (Bind::Pair(first, second), Item::Pair(key, value)) => {
                self.push_local(first, key);
                self.push_local(second, value);
            }
            // a pair pattern over a scalar item never renders, the names just need a slot
            (Bind::Pair(first, second), Item::Scalar(ty)) => {
                self.push_local(first, ty);
                self.push_local(second, ty);
            }
        }
    }
}
