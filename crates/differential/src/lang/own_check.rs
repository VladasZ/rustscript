//! Replays a finished block against the ownership rules of `own`, see `check_block`.

use std::collections::BTreeSet;

use crate::lang::block::{Block, FnKind};
use crate::lang::expr::{Expr, MemKind, ReadMode};
use crate::lang::own::{BindKind, Scope, referenced};
use crate::lang::pat::Pat;
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

pub(super) struct Checker {
    pub(super) scope: Scope,
    /// the first rule broken
    fault: Option<String>,
    /// The reference being checked is kept by a `let`, so the binding it borrows is held to
    /// the end of the scope and no temporary may stand behind it.
    pub(super) bound: bool,
    /// A temporary may be borrowed, the reference is used up before the temporary ends. It
    /// is not once the reference leaves a closure body, a match arm, an `if` branch or a
    /// block, each ends its own temporaries.
    temps_ok: bool,
}

impl Default for Checker {
    fn default() -> Self {
        Self {
            scope: Scope::default(),
            fault: None,
            bound: false,
            temps_ok: true,
        }
    }
}

impl Checker {
    pub(super) fn push_local(&mut self, name: &str, ty: &Ty) {
        self.scope
            .push(name.to_string(), ty.clone(), BindKind::Local);
    }

    /// The bindings of a pattern. A `ref` one stands behind a reference.
    pub(super) fn push_pat(&mut self, pat: &Pat) {
        let mut binds = Vec::new();
        pat.bindings(&mut binds);
        let borrowed = pat.borrowed();
        for (name, ty) in binds {
            if borrowed.contains(&name) {
                self.scope.push_borrowed(name, ty);
            } else {
                self.scope.push(name, ty, BindKind::Local);
            }
        }
    }

    /// The bindings of a pattern over `scrutinee`. A `ref` binding into a place keeps that
    /// place's binding borrowed until the scope ends, see `Pat::pins`.
    pub(super) fn push_matched(&mut self, pat: &Pat, scrutinee: &Expr) {
        if let Some(root) = pat.pins(scrutinee) {
            self.scope.pin(root);
        }
        self.push_pat(pat);
    }

    pub(super) fn push_let(&mut self, name: &str, ty: &Ty) {
        self.scope.push_let(name.to_string(), ty.clone());
    }

    pub(super) fn require(&mut self, condition: bool, what: impl FnOnce() -> String) {
        if !condition && self.fault.is_none() {
            self.fault = Some(what());
        }
    }

    pub(super) fn stmts(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            self.stmt(stmt);
        }
    }

    /// A body of its own scope, its bindings gone at the end.
    pub(super) fn body(&mut self, stmts: &[Stmt]) {
        let mark = self.scope.enter_scope();
        self.stmts(stmts);
        self.scope.exit_scope(mark);
    }

    /// A loop body. Nothing it revives counts after it, the loop may run zero times.
    fn loop_body(&mut self, stmts: &[Stmt]) {
        let before = self.scope.snapshot();
        self.scope.enter_loop();
        self.body(stmts);
        self.scope.leave_loop();
        self.scope.restore(&before);
    }

    pub(super) fn branches(&mut self, count: usize, mut build: impl FnMut(&mut Self, usize)) {
        let before = self.scope.snapshot();
        let mut ends = Vec::with_capacity(count);
        for index in 0..count {
            self.scope.restore(&before);
            let mark = self.scope.enter_scope();
            build(self, index);
            self.scope.exit_scope(mark);
            ends.push(self.scope.snapshot());
        }
        self.scope.merge(&before, &ends);
    }

    /// A reference an expression takes lives until its statement ends.
    fn stmt(&mut self, stmt: &Stmt) {
        let mark = self.scope.stmt_mark();
        let saved = (self.bound, self.temps_ok);
        (self.bound, self.temps_ok) = (false, true);
        self.stmt_inner(stmt);
        (self.bound, self.temps_ok) = saved;
        self.scope.stmt_release(mark);
    }

    /// An expression whose value a `let` keeps. With a reference in its type it runs bound,
    /// see `Checker::bound`.
    pub(super) fn kept_expr(&mut self, expr: &Expr, ty: &Ty) {
        if !ty.contains_ref() {
            self.expr(expr);
            return;
        }
        let saved = (self.bound, self.temps_ok);
        (self.bound, self.temps_ok) = (true, false);
        self.expr(expr);
        (self.bound, self.temps_ok) = saved;
    }

    /// The value leaves a scope that ends its own temporaries.
    fn crossing(&mut self, build: impl FnOnce(&mut Self)) {
        let saved = std::mem::replace(&mut self.temps_ok, false);
        build(self);
        self.temps_ok = saved;
    }

    /// A reference out of `base`, see `Expr::Borrow`.
    fn borrow(&mut self, base: &Expr) {
        match base {
            Expr::Var { name, ty, .. } if !ty.contains_ref() => {
                self.require(self.scope.can_borrow(name), || {
                    format!("borrow of `{name}`")
                });
                if self.bound {
                    self.scope.pin(name);
                } else {
                    self.scope.borrow_for_stmt(name);
                }
            }
            other => {
                let lends = other.ty().contains_ref();
                self.require(lends || self.temps_ok, || {
                    "a reference outlives the temporary it borrows".to_string()
                });
                self.expr(other);
            }
        }
    }

    fn stmt_inner(&mut self, stmt: &Stmt) {
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
                self.require(self.scope.can_write(name), || format!("mutate `{name}`"));
                self.scope.freeze(name);
                self.mut_op(name, op);
                self.scope.unfreeze();
            }
            Stmt::ForAccum {
                var,
                source,
                target,
                op,
            } => {
                self.require(self.scope.can_write(target), || {
                    format!("accumulate into `{target}`")
                });
                self.scope.freeze(target);
                self.expr(source);
                let elem = match source.ty() {
                    Ty::Vec(elem) => *elem,
                    other => other,
                };
                self.loop_with(var, &elem, |inner| inner.mut_op(target, op));
                self.scope.unfreeze();
            }
            Stmt::ForMut {
                name,
                var,
                elem,
                expr,
            } => {
                self.require(self.scope.can_write(name), || {
                    format!("iter_mut over `{name}`")
                });
                let hidden = self.scope.hide(name);
                self.loop_with(var, elem, |inner| inner.expr(expr));
                if let Some(hidden) = hidden {
                    self.scope.unhide(hidden);
                }
            }
            Stmt::CallMut { name, args, .. } => {
                self.require(self.scope.can_write(name), || format!("&mut of `{name}`"));
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
            Stmt::IfLet { .. }
            | Stmt::WhileLet { .. }
            | Stmt::LetElse { .. }
            | Stmt::Match { .. }
            | Stmt::LetLoop { .. } => self.binding_form(stmt),
        }
    }

    /// The statements that declare or write a binding.
    fn binding_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { name, ty, expr, .. } => {
                self.kept_expr(expr, ty);
                // a binding that holds a reference is never written, so what it borrows
                // stays the same for its whole life
                if ty.contains_ref() {
                    self.push_local(name, ty);
                } else {
                    self.push_let(name, ty);
                }
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
                // each call is a statement of its own
                for call in calls {
                    let mark = self.scope.stmt_mark();
                    self.expr(call);
                    self.scope.stmt_release(mark);
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
                self.require(self.scope.can_compound(name), || {
                    format!("compound on `{name}`")
                });
                self.scope.freeze(name);
                self.expr(expr);
                self.scope.unfreeze();
            }
            _ => unreachable!("binding_stmt handles the binding statements only"),
        }
    }

    /// A loop over items bound to `var`, see `loop_body`.
    fn loop_with(&mut self, var: &str, elem: &Ty, build: impl FnOnce(&mut Self)) {
        let before = self.scope.snapshot();
        self.scope.enter_loop();
        let mark = self.scope.enter_scope();
        self.push_local(var, elem);
        build(self);
        self.scope.exit_scope(mark);
        self.scope.leave_loop();
        self.scope.restore(&before);
    }

    /// `target` is the binding the op writes, the retain binding takes its element type.
    fn mut_op(&mut self, target: &str, op: &MutOp) {
        if let MutOp::VecRetain { bind, pred } = op {
            let elem = match self.scope.slot(target).map(|slot| slot.ty.clone()) {
                Some(Ty::Vec(elem)) => *elem,
                other => {
                    self.require(false, || format!("retain on `{target}` of type {other:?}"));
                    return;
                }
            };
            self.scope.enter_closure();
            let mark = self.scope.enter_scope();
            self.push_local(bind, &elem);
            self.expr(pred);
            self.scope.exit_scope(mark);
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
                let mark = self.scope.enter_scope();
                // the closure owns its captures, so inside they are fresh locals
                for (used, ty) in &captured {
                    self.push_local(used, ty);
                }
                for param in params {
                    for (local, ty) in param.locals() {
                        self.push_local(&local, &ty);
                    }
                }
                self.crossing(|inner| inner.expr(body));
                self.scope.exit_scope(mark);
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

    /// A value with no reference in its type uses up every reference below it, so the rules
    /// for a kept or a leaving reference end there.
    pub(super) fn expr(&mut self, expr: &Expr) {
        let strict = self.bound || !self.temps_ok;
        if strict && !expr.ty().contains_ref() {
            let saved = (self.bound, self.temps_ok);
            (self.bound, self.temps_ok) = (false, true);
            self.expr_inner(expr);
            (self.bound, self.temps_ok) = saved;
        } else {
            self.expr_inner(expr);
        }
    }

    fn expr_inner(&mut self, expr: &Expr) {
        match expr {
            Expr::Borrow { base, .. } => self.borrow(base),
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
                    inner.push_matched(&arm.pat, scrutinee);
                    if let Some(guard) = &arm.guard {
                        inner.expr(guard);
                    }
                    inner.crossing(|inner| inner.expr(&arm.body));
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
                    let side = if index == 0 { then_expr } else { else_expr };
                    inner.crossing(|inner| inner.expr(side));
                });
            }
            Expr::Matches {
                scrutinee,
                pat,
                guard,
            } => self.matches(scrutinee, pat, guard.as_deref()),
            Expr::Block { stmts, tail } => {
                let mark = self.scope.enter_scope();
                self.stmts(stmts);
                self.crossing(|inner| inner.expr(tail));
                self.scope.exit_scope(mark);
            }
            Expr::Pipe(pipe) => self.pipe(pipe),
            _ => {
                for child in expr.children() {
                    self.expr(child);
                }
            }
        }
    }

    /// `matches!`, the guard alone sees the bindings.
    fn matches(&mut self, scrutinee: &Expr, pat: &Pat, guard: Option<&Expr>) {
        self.expr(scrutinee);
        let mark = self.scope.enter_scope();
        self.push_pat(pat);
        if let Some(guard) = guard {
            self.expr(guard);
        }
        self.scope.exit_scope(mark);
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
                let mark = self.scope.enter_scope();
                self.push_bind(acc, &acc_item);
                self.push_bind(bind, &item);
                self.crossing(|inner| inner.expr(body));
                self.scope.exit_scope(mark);
                self.scope.leave_closure();
            }
            _ => {}
        }
    }

    fn pipe_body(&mut self, bind: &Bind, item: &Item, body: &Expr) {
        self.scope.enter_closure();
        let mark = self.scope.enter_scope();
        self.push_bind(bind, item);
        self.crossing(|inner| inner.expr(body));
        self.scope.exit_scope(mark);
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
