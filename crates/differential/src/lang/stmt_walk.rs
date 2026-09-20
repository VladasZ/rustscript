//! Walks over a `Stmt`. Every walk goes through `bodies` and `exprs`, so a new statement kind is
//! handled in 1 place.

use std::collections::BTreeSet;

use crate::lang::expr::{Expr, Helper};
use crate::lang::pat::Pat;

use super::stmt::{Ann, ChainLink, ClosureParam, ClosureSource, Exit, Stmt};

impl Stmt {
    /// Whether a `break` or `continue` in here, at any depth, names `label`.
    pub fn targets_label(&self, label: &str) -> bool {
        match self {
            Self::Break { label: Some(l), .. } | Self::Continue { label: Some(l), .. } => {
                l == label
            }
            Self::LetElse {
                exit: Exit::Break(Some(l)) | Exit::Continue(Some(l)),
                ..
            } if l == label => true,
            _ => self
                .bodies()
                .iter()
                .any(|body| body.iter().any(|stmt| stmt.targets_label(label))),
        }
    }

    pub fn bodies(&self) -> Vec<&Vec<Stmt>> {
        match self {
            Self::If {
                then_body,
                else_body,
                ..
            } => vec![then_body, else_body],
            Self::ForRange { body, .. }
            | Self::While { body, .. }
            | Self::Loop { body, .. }
            | Self::Scope { body }
            | Self::WhileLet { body, .. }
            | Self::LetLoop { body, .. }
            | Self::LetElse {
                else_body: body, ..
            } => vec![body],
            Self::IfLet {
                then_body,
                else_body,
                ..
            } => {
                let mut out = vec![then_body];
                out.extend(else_body.iter());
                out
            }
            Self::Match { arms, .. } => arms.iter().map(|arm| &arm.body).collect(),
            _ => Vec::new(),
        }
    }

    pub(super) fn bodies_mut(&mut self) -> Vec<&mut Vec<Stmt>> {
        match self {
            Self::If {
                then_body,
                else_body,
                ..
            } => vec![then_body, else_body],
            Self::ForRange { body, .. }
            | Self::While { body, .. }
            | Self::Loop { body, .. }
            | Self::Scope { body }
            | Self::WhileLet { body, .. }
            | Self::LetLoop { body, .. }
            | Self::LetElse {
                else_body: body, ..
            } => vec![body],
            Self::IfLet {
                then_body,
                else_body,
                ..
            } => {
                let mut out = vec![then_body];
                out.extend(else_body.iter_mut());
                out
            }
            Self::Match { arms, .. } => arms.iter_mut().map(|arm| &mut arm.body).collect(),
            _ => Vec::new(),
        }
    }

    /// The expressions of this statement alone, nested bodies excluded.
    pub fn own_exprs(&self) -> Vec<&Expr> {
        let mut out = Vec::new();
        match self {
            Self::Let { expr, .. }
            | Self::LetTuple { expr, .. }
            | Self::Assign { expr, .. }
            | Self::AssignField { expr, .. }
            | Self::Compound { expr, .. }
            | Self::Print { expr, .. }
            | Self::ForMut { expr, .. } => out.push(expr),
            Self::LetClosure { source, calls, .. } => {
                match source {
                    ClosureSource::Literal { body, .. } => out.push(body),
                    ClosureSource::Factory { arg, .. } => out.push(arg),
                }
                out.extend(calls.iter());
            }
            Self::If { condition, .. }
            | Self::Break { condition, .. }
            | Self::Continue { condition, .. } => out.push(condition),
            Self::Return { condition, value } => {
                out.push(condition);
                out.push(value);
            }
            Self::Mutate { op, .. } => out.extend(op.exprs()),
            Self::ForAccum { source, op, .. } => {
                out.push(source);
                out.extend(op.exprs());
            }
            Self::CallMut { args, .. } => out.extend(args.iter()),
            Self::IfLet { links, .. } => out.extend(links.iter().map(ChainLink::expr)),
            Self::LetElse { expr, exit, .. } => {
                out.push(expr);
                if let Exit::Return(Some(value)) = exit {
                    out.push(value);
                }
            }
            Self::Match {
                scrutinee, arms, ..
            } => {
                out.push(scrutinee);
                out.extend(arms.iter().filter_map(|arm| arm.guard.as_ref()));
            }
            Self::LetLoop {
                exit,
                value,
                fallback,
                ..
            } => out.extend([exit, value, fallback]),
            Self::ForRange { .. }
            | Self::While { .. }
            | Self::Loop { .. }
            | Self::Swap { .. }
            | Self::Scope { .. }
            | Self::WhileLet { .. } => {}
        }
        out
    }

    /// Every expression, in a fixed order shared with `exprs_mut`, the statement's own first
    /// and then each body in `bodies` order.
    pub fn exprs(&self) -> Vec<&Expr> {
        let mut out = self.own_exprs();
        for body in self.bodies() {
            for stmt in body {
                out.extend(stmt.exprs());
            }
        }
        out
    }

    pub fn exprs_mut(&mut self) -> Vec<&mut Expr> {
        let mut out = Vec::new();
        match self {
            Self::Let { expr, .. }
            | Self::LetTuple { expr, .. }
            | Self::Assign { expr, .. }
            | Self::AssignField { expr, .. }
            | Self::Compound { expr, .. }
            | Self::Print { expr, .. }
            | Self::ForMut { expr, .. } => out.push(expr),
            Self::LetClosure { source, calls, .. } => {
                match source {
                    ClosureSource::Literal { body, .. } => out.push(body),
                    ClosureSource::Factory { arg, .. } => out.push(arg),
                }
                out.extend(calls.iter_mut());
            }
            Self::If {
                condition,
                then_body,
                else_body,
            } => {
                out.push(condition);
                for stmt in then_body.iter_mut().chain(else_body.iter_mut()) {
                    out.extend(stmt.exprs_mut());
                }
            }
            Self::Break { condition, .. } | Self::Continue { condition, .. } => {
                out.push(condition);
            }
            Self::Return { condition, value } => {
                out.push(condition);
                out.push(value);
            }
            Self::Mutate { op, .. } => out.extend(op.exprs_mut()),
            Self::ForAccum { source, op, .. } => {
                out.push(source);
                out.extend(op.exprs_mut());
            }
            Self::CallMut { args, .. } => out.extend(args.iter_mut()),
            Self::Swap { .. } => {}
            Self::ForRange { body, .. }
            | Self::While { body, .. }
            | Self::Loop { body, .. }
            | Self::Scope { body }
            | Self::WhileLet { body, .. } => {
                for stmt in body {
                    out.extend(stmt.exprs_mut());
                }
            }
            Self::IfLet {
                links,
                then_body,
                else_body,
            } => {
                out.extend(links.iter_mut().map(ChainLink::expr_mut));
                for stmt in then_body.iter_mut().chain(else_body.iter_mut().flatten()) {
                    out.extend(stmt.exprs_mut());
                }
            }
            Self::LetElse {
                expr,
                exit,
                else_body,
                ..
            } => {
                out.push(expr);
                if let Exit::Return(Some(value)) = exit {
                    out.push(value);
                }
                for stmt in else_body {
                    out.extend(stmt.exprs_mut());
                }
            }
            Self::Match {
                scrutinee, arms, ..
            } => {
                out.push(scrutinee);
                let mut bodies = Vec::new();
                for arm in arms {
                    out.extend(arm.guard.iter_mut());
                    bodies.push(&mut arm.body);
                }
                for stmt in bodies.into_iter().flatten() {
                    out.extend(stmt.exprs_mut());
                }
            }
            Self::LetLoop {
                exit,
                value,
                fallback,
                body,
                ..
            } => {
                out.extend([exit, value, fallback]);
                for stmt in body {
                    out.extend(stmt.exprs_mut());
                }
            }
        }
        out
    }

    /// Names this statement writes to, so the renderer knows which bindings need `mut`.
    pub fn assigned(&self, out: &mut BTreeSet<String>) {
        match self {
            Self::Assign { name, .. }
            | Self::AssignField { name, .. }
            | Self::Compound { name, .. }
            | Self::Mutate { name, .. }
            | Self::ForMut { name, .. }
            | Self::CallMut { name, .. }
            | Self::WhileLet { name, .. }
            | Self::ForAccum { target: name, .. }
            | Self::LetClosure {
                name,
                source: ClosureSource::Literal { mutates: true, .. },
                ..
            } => {
                out.insert(name.clone());
            }
            // a body that hands a captured closure to the apply helper borrows it by `&mut`,
            // so the closure is `FnMut` and its binding needs `mut` to be called
            Self::LetClosure {
                name,
                source: ClosureSource::Literal { body, .. },
                ..
            } if body
                .nodes()
                .iter()
                .any(|node| matches!(node, Expr::ApplyCall { .. })) =>
            {
                out.insert(name.clone());
            }
            Self::Swap { a, b } => {
                out.insert(a.clone());
                out.insert(b.clone());
            }
            _ => {}
        }
        for body in self.bodies() {
            for stmt in body {
                stmt.assigned(out);
            }
        }
        for expr in self.exprs() {
            expr.written_names(out);
            for node in expr.nodes() {
                if let Expr::Block { stmts, .. } = node {
                    for stmt in stmts {
                        stmt.assigned(out);
                    }
                }
            }
        }
    }

    /// The names this statement alone writes, nested bodies excluded. `mark_mutable` resolves
    /// each against the scope at the statement.
    pub fn own_writes(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        match self {
            Self::Assign { name, .. }
            | Self::AssignField { name, .. }
            | Self::Compound { name, .. }
            | Self::Mutate { name, .. }
            | Self::ForMut { name, .. }
            | Self::CallMut { name, .. }
            | Self::WhileLet { name, .. }
            | Self::ForAccum { target: name, .. } => {
                out.insert(name.clone());
            }
            Self::Swap { a, b } => {
                out.insert(a.clone());
                out.insert(b.clone());
            }
            _ => {}
        }
        for expr in self.own_exprs() {
            expr.written_names(&mut out);
            for node in expr.nodes() {
                if let Expr::Block { stmts, .. } = node {
                    for stmt in stmts {
                        stmt.assigned(&mut out);
                    }
                }
            }
        }
        out
    }

    /// The binding a compound assignment writes.
    pub fn declared_targets(&self) -> Vec<String> {
        match self {
            Self::Compound { name, .. } | Self::Assign { name, .. } => vec![name.clone()],
            _ => Vec::new(),
        }
    }

    pub fn declared(&self) -> Vec<String> {
        match self {
            Self::Let { name, .. } | Self::LetClosure { name, .. } | Self::LetLoop { name, .. } => {
                vec![name.clone()]
            }
            Self::LetTuple { names, .. } => names.iter().map(|(n, _)| n.clone()).collect(),
            Self::LetElse { pat, .. } => {
                let mut binds = Vec::new();
                pat.bindings(&mut binds);
                binds.into_iter().map(|(name, _)| name).collect()
            }
            _ => Vec::new(),
        }
    }

    /// The names a statement writes without declaring them. When it goes, a later read of a
    /// moved binding it revived goes with it, see `remove_with_dependents`.
    pub fn revives(&self) -> Vec<String> {
        match self {
            Self::Assign { name, .. } | Self::AssignField { name, .. } => vec![name.clone()],
            _ => Vec::new(),
        }
    }

    pub fn uses_any(&self, names: &BTreeSet<String>) -> bool {
        let direct = match self {
            Self::LetClosure {
                source: ClosureSource::Factory { fn_name, .. },
                ..
            } => names.contains(fn_name),
            _ => false,
        };
        direct || self.exprs().iter().any(|expr| expr.uses_any(names))
    }

    /// Whether this statement assigns to a name it doesn't own, so the reducer can't drop that binding.
    pub fn writes_any(&self, names: &BTreeSet<String>) -> bool {
        let mut written = BTreeSet::new();
        self.assigned(&mut written);
        written.iter().any(|name| names.contains(name))
    }

    pub fn has_fallible_op(&self) -> bool {
        let own = match self {
            Self::Compound { op, .. } => op.is_fallible(),
            Self::Mutate { op, .. } | Self::ForAccum { op, .. } => op.has_fallible_op(),
            _ => false,
        };
        own || self.own_exprs().iter().any(|expr| expr.has_fallible_op())
            || self
                .bodies()
                .iter()
                .any(|body| body.iter().any(Stmt::has_fallible_op))
    }

    pub fn make_opaque(&mut self) {
        for expr in self.exprs_mut() {
            expr.make_opaque();
        }
    }

    pub fn helpers(&self, out: &mut BTreeSet<Helper>) {
        for expr in self.exprs() {
            expr.helpers(out);
        }
    }

    pub fn features(&self, out: &mut BTreeSet<&'static str>) {
        let own: &[&'static str] = match self {
            Self::Let {
                ann: Ann::Typed, ..
            } => &["lang-let"],
            Self::Let {
                ann: Ann::Inferred, ..
            } => &["lang-let", "lang-let-inferred"],
            Self::LetTuple { .. } => &["lang-let-tuple"],
            Self::LetClosure { source, .. } => match source {
                ClosureSource::Literal {
                    capture_move: true,
                    mutates: true,
                    ..
                } => &["lang-closure", "lang-closure-move", "lang-closure-mut"],
                ClosureSource::Literal {
                    capture_move: true, ..
                } => &["lang-closure", "lang-closure-move"],
                ClosureSource::Literal { mutates: true, .. } => {
                    &["lang-closure", "lang-closure-mut"]
                }
                ClosureSource::Literal { .. } => &["lang-closure"],
                ClosureSource::Factory { .. } => &["lang-closure", "lang-closure-factory"],
            },
            Self::Assign { .. } => &["lang-assign"],
            Self::AssignField { .. } => &["lang-assign-field"],
            Self::Swap { .. } => &["lang-mem-swap"],
            Self::Scope { .. } => &["lang-scope"],
            Self::Compound { .. } => &["lang-compound"],
            Self::Print { .. } | Self::Mutate { .. } => &[],
            Self::If { .. } => &["lang-if-stmt"],
            Self::ForRange { label: Some(_), .. } => &["lang-for", "lang-loop-label"],
            Self::ForRange { .. } => &["lang-for"],
            Self::While { label: Some(_), .. } => &["lang-while", "lang-loop-label"],
            Self::While { .. } => &["lang-while"],
            Self::Loop { label: Some(_), .. } => &["lang-loop", "lang-loop-label"],
            Self::Loop { .. } => &["lang-loop"],
            Self::Break { label: Some(_), .. } => &["lang-break", "lang-break-label"],
            Self::Break { .. } => &["lang-break"],
            Self::Continue { label: Some(_), .. } => &["lang-continue", "lang-continue-label"],
            Self::Continue { .. } => &["lang-continue"],
            Self::Return { .. } => &["lang-early-return"],
            Self::ForAccum { .. } => &["lang-for-accum"],
            Self::ForMut { .. } => &["lang-iter-mut"],
            Self::CallMut { .. } => &["lang-borrow-mut"],
            Self::IfLet {
                links,
                else_body: Some(_),
                ..
            } if links.len() > 1 => &["lang-if-let", "lang-if-let-else", "lang-let-chain"],
            Self::IfLet { links, .. } if links.len() > 1 => &["lang-if-let", "lang-let-chain"],
            Self::IfLet {
                else_body: Some(_), ..
            } => &["lang-if-let", "lang-if-let-else"],
            Self::IfLet { .. } => &["lang-if-let"],
            Self::WhileLet { .. } => &["lang-while-let"],
            Self::LetElse { .. } => &["lang-let-else"],
            Self::Match { .. } => &["lang-match-stmt"],
            Self::LetLoop { .. } => &["lang-loop-break-value"],
        };
        out.extend(own.iter().copied());
        if let Self::LetClosure {
            source: ClosureSource::Literal { params, .. },
            ..
        } = self
            && params
                .iter()
                .any(|param| matches!(param, ClosureParam::Pair { .. }))
        {
            out.insert("lang-closure-tuple-param");
        }
        for pat in self.pats() {
            pat.features(out);
        }
        if let Self::Match { arms, .. } = self
            && arms.iter().any(|arm| arm.guard.is_some())
        {
            out.insert("lang-pat-guard");
        }
        match self {
            Self::Print { spec, form, .. } => {
                spec.features(out);
                out.insert(form.feature());
            }
            Self::Compound { op, .. } => {
                out.insert(op.feature());
            }
            Self::Mutate { op, .. } | Self::ForAccum { op, .. } => {
                out.insert(op.feature());
            }
            _ => {}
        }
        for body in self.bodies() {
            for stmt in body {
                stmt.features(out);
            }
        }
        for expr in self.exprs() {
            expr.features(out);
        }
    }

    pub fn shape(&self, out: &mut String) {
        match self {
            Self::Let { .. } => out.push_str("let,"),
            Self::LetTuple { .. } => out.push_str("let-tuple,"),
            Self::LetClosure { .. } => out.push_str("closure,"),
            Self::Assign { .. } => out.push_str("assign,"),
            Self::AssignField { .. } => out.push_str("assign-field,"),
            Self::Swap { .. } => out.push_str("swap,"),
            Self::Scope { body } => {
                out.push_str("scope(");
                for stmt in body {
                    stmt.shape(out);
                }
                out.push_str("),");
            }
            Self::Compound { .. } => out.push_str("compound,"),
            Self::Print { .. } => out.push_str("print,"),
            Self::If {
                then_body,
                else_body,
                ..
            } => {
                out.push_str("if(");
                for stmt in then_body {
                    stmt.shape(out);
                }
                out.push('|');
                for stmt in else_body {
                    stmt.shape(out);
                }
                out.push_str("),");
            }
            Self::ForRange { body, .. } | Self::While { body, .. } | Self::Loop { body, .. } => {
                out.push_str(match self {
                    Self::ForRange { .. } => "for(",
                    Self::While { .. } => "while(",
                    _ => "loop(",
                });
                for stmt in body {
                    stmt.shape(out);
                }
                out.push_str("),");
            }
            Self::Break { .. } => out.push_str("break,"),
            Self::Continue { .. } => out.push_str("continue,"),
            Self::Return { .. } => out.push_str("return,"),
            Self::Mutate { op, .. } => {
                out.push_str("mutate:");
                out.push_str(op.feature());
                out.push(',');
            }
            Self::ForAccum { op, .. } => {
                out.push_str("for-accum:");
                out.push_str(op.feature());
                out.push(',');
            }
            Self::ForMut { .. } => out.push_str("for-mut,"),
            Self::CallMut { .. } => out.push_str("call-mut,"),
            Self::IfLet { .. }
            | Self::WhileLet { .. }
            | Self::LetElse { .. }
            | Self::Match { .. }
            | Self::LetLoop { .. } => {
                out.push_str(match self {
                    Self::IfLet { .. } => "if-let(",
                    Self::WhileLet { .. } => "while-let(",
                    Self::LetElse { .. } => "let-else(",
                    Self::Match { .. } => "match(",
                    _ => "let-loop(",
                });
                for body in self.bodies() {
                    for stmt in body {
                        stmt.shape(out);
                    }
                    out.push('|');
                }
                out.push_str("),");
            }
        }
    }

    /// The patterns this statement matches against, nested bodies excluded.
    pub fn pats(&self) -> Vec<&Pat> {
        match self {
            Self::IfLet { links, .. } => links
                .iter()
                .filter_map(|link| match link {
                    ChainLink::Let { pat, .. } => Some(pat),
                    ChainLink::Cond(_) => None,
                })
                .collect(),
            Self::WhileLet { pat, .. } | Self::LetElse { pat, .. } => vec![pat],
            Self::Match { arms, .. } => arms.iter().map(|arm| &arm.pat).collect(),
            _ => Vec::new(),
        }
    }
}
