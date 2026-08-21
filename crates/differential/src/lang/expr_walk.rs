//! Walks over the expression tree: free variables, laundering, helpers,
//! coverage features, and shrinking. Every walk goes through `children`, so
//! a new node kind is handled in one place.

use std::collections::{BTreeMap, BTreeSet};

use crate::lang::expr::{Arm, BinOp, Expr, Helper, UnOp, lookup, minimal};
use crate::lang::stmt::Stmt;
use crate::lang::ty::FloatWidth;

/// How many shrinks of one child are tried in the parent's candidate list.
/// The reducer takes the first improvement, so a short list keeps each
/// round cheap without losing the deep ones, which the next round reaches.
const CHILD_SHRINKS: usize = 3;

impl Expr {
    /// Direct children in a fixed order, the same order `children_mut`
    /// walks, so a shrink can rewrite the child it inspected.
    pub fn children(&self) -> Vec<&Expr> {
        match self {
            Self::Bin { left, right, .. } => vec![left, right],
            Self::Unary { value, .. }
            | Self::Cast { value, .. }
            | Self::Try { value, .. }
            | Self::Into { value, .. }
            | Self::ApplyCall { arg: value, .. } => vec![value],
            Self::Call { recv, args, .. } => {
                let mut out = vec![&**recv];
                out.extend(args.iter());
                out
            }
            Self::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => vec![condition, then_expr, else_expr],
            Self::VecLit { items, .. } | Self::SetLit { items, .. } | Self::TupleLit(items) => {
                items.iter().collect()
            }
            Self::OptLit { value, .. } => value.iter().map(|inner| &**inner).collect(),
            Self::MapLit { items, .. } => {
                items.iter().flat_map(|(key, value)| [key, value]).collect()
            }
            Self::ResLit { value, .. } => match value {
                Ok(inner) | Err(inner) => vec![inner],
            },
            Self::StructLit { fields, .. } => fields.iter().collect(),
            Self::EnumLit { payload, .. } => payload.iter().collect(),
            Self::Pipe(pipe) => pipe.exprs(),
            Self::FnCall { args, .. } | Self::ClosureCall { args, .. } => args.iter().collect(),
            Self::Field { base, .. } | Self::TupleField { base, .. } | Self::TraitCall { base } => {
                vec![base]
            }
            Self::Index { base, index, .. } => vec![base, index],
            Self::Method { base, args, .. } => {
                let mut out: Vec<&Expr> = base.iter().map(|b| &**b).collect();
                out.extend(args.iter());
                out
            }
            Self::Match {
                scrutinee, arms, ..
            } => {
                let mut out = vec![&**scrutinee];
                for arm in arms {
                    out.extend(arm.guard.iter());
                    out.push(&arm.body);
                }
                out
            }
            Self::Block { stmts, tail } => {
                let mut out: Vec<&Expr> = stmts.iter().flat_map(|s| s.exprs()).collect();
                out.push(tail);
                out
            }
            Self::IntLit { .. }
            | Self::BareInt { .. }
            | Self::FloatLit { .. }
            | Self::BareFloat { .. }
            | Self::BoolLit { .. }
            | Self::CharLit { .. }
            | Self::StrLit(_)
            | Self::StdErrLit(_)
            | Self::DefaultOf(_)
            | Self::Var { .. }
            | Self::ConstRef { .. } => Vec::new(),
        }
    }

    pub fn children_mut(&mut self) -> Vec<&mut Expr> {
        match self {
            Self::Bin { left, right, .. } => vec![left, right],
            Self::Unary { value, .. }
            | Self::Cast { value, .. }
            | Self::Try { value, .. }
            | Self::Into { value, .. }
            | Self::ApplyCall { arg: value, .. } => vec![value],
            Self::Call { recv, args, .. } => {
                let mut out = vec![&mut **recv];
                out.extend(args.iter_mut());
                out
            }
            Self::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => vec![condition, then_expr, else_expr],
            Self::VecLit { items, .. } | Self::SetLit { items, .. } | Self::TupleLit(items) => {
                items.iter_mut().collect()
            }
            Self::OptLit { value, .. } => value.iter_mut().map(|inner| &mut **inner).collect(),
            Self::MapLit { items, .. } => items
                .iter_mut()
                .flat_map(|(key, value)| [key, value])
                .collect(),
            Self::ResLit { value, .. } => match value {
                Ok(inner) | Err(inner) => vec![inner],
            },
            Self::StructLit { fields, .. } => fields.iter_mut().collect(),
            Self::EnumLit { payload, .. } => payload.iter_mut().collect(),
            Self::Pipe(pipe) => pipe.exprs_mut(),
            Self::FnCall { args, .. } | Self::ClosureCall { args, .. } => args.iter_mut().collect(),
            Self::Field { base, .. } | Self::TupleField { base, .. } | Self::TraitCall { base } => {
                vec![base]
            }
            Self::Index { base, index, .. } => vec![base, index],
            Self::Method { base, args, .. } => {
                let mut out: Vec<&mut Expr> = base.iter_mut().map(|b| &mut **b).collect();
                out.extend(args.iter_mut());
                out
            }
            Self::Match {
                scrutinee, arms, ..
            } => {
                let mut out = vec![&mut **scrutinee];
                for arm in arms {
                    out.extend(arm.guard.iter_mut());
                    out.push(&mut arm.body);
                }
                out
            }
            Self::Block { stmts, tail } => {
                let mut out: Vec<&mut Expr> =
                    stmts.iter_mut().flat_map(|s| s.exprs_mut()).collect();
                out.push(tail);
                out
            }
            Self::IntLit { .. }
            | Self::BareInt { .. }
            | Self::FloatLit { .. }
            | Self::BareFloat { .. }
            | Self::BoolLit { .. }
            | Self::CharLit { .. }
            | Self::StrLit(_)
            | Self::StdErrLit(_)
            | Self::DefaultOf(_)
            | Self::Var { .. }
            | Self::ConstRef { .. } => Vec::new(),
        }
    }

    /// Every node in the tree, this one first, in a stable pre-order.
    pub fn nodes(&self) -> Vec<&Expr> {
        let mut out = vec![self];
        for child in self.children() {
            out.extend(child.nodes());
        }
        out
    }

    /// The `index`th node of `nodes`, mutably.
    pub fn nth_node_mut(&mut self, index: usize) -> Option<&mut Expr> {
        let mut remaining = index;
        self.nth_node_inner(&mut remaining)
    }

    fn nth_node_inner(&mut self, remaining: &mut usize) -> Option<&mut Expr> {
        if *remaining == 0 {
            return Some(self);
        }
        *remaining -= 1;
        for child in self.children_mut() {
            if let Some(found) = child.nth_node_inner(remaining) {
                return Some(found);
            }
        }
        None
    }

    pub fn uses_any(&self, names: &BTreeSet<String>) -> bool {
        match self {
            Self::Var { name, .. } => names.contains(name),
            Self::ClosureCall { name, args, .. } => {
                names.contains(name) || args.iter().any(|arg| arg.uses_any(names))
            }
            Self::ApplyCall { closure, arg, .. } => names.contains(closure) || arg.uses_any(names),
            Self::Block { stmts, tail } => {
                stmts.iter().any(|stmt| stmt.uses_any(names)) || tail.uses_any(names)
            }
            _ => self.children().iter().any(|child| child.uses_any(names)),
        }
    }

    /// Whether the subtree contains an operator that can abort at runtime.
    /// Drives the decision to launder integer literals.
    pub fn has_fallible_op(&self) -> bool {
        match self {
            Self::Bin { op, .. } if op.is_fallible() => true,
            // Every call reaches code the const propagator can look through,
            // `pow` and `sum` among them, so a call counts as fallible. A
            // pipe carries `sum` and `fold`, a helper body is out of sight,
            // an index can miss, all count the same way.
            Self::Unary { op: UnOp::Neg, .. }
            | Self::Call { .. }
            | Self::Pipe(_)
            | Self::FnCall { .. }
            | Self::ClosureCall { .. }
            | Self::ApplyCall { .. }
            | Self::Method { .. }
            | Self::Index { .. } => true,
            Self::Block { stmts, tail } => {
                stmts.iter().any(Stmt::has_fallible_op) || tail.has_fallible_op()
            }
            _ => self.children().iter().any(|child| child.has_fallible_op()),
        }
    }

    /// Mark every literal in the subtree as needing the opaque helper.
    pub fn make_opaque(&mut self) {
        match self {
            Self::IntLit { opaque, .. }
            | Self::FloatLit { opaque, .. }
            | Self::BareInt { opaque, .. }
            | Self::BareFloat { opaque, .. }
            | Self::BoolLit { opaque, .. }
            | Self::CharLit { opaque, .. }
            | Self::ConstRef { opaque, .. } => *opaque = true,
            Self::Block { stmts, tail } => {
                for stmt in stmts {
                    stmt.make_opaque();
                }
                tail.make_opaque();
            }
            _ => {
                for child in self.children_mut() {
                    child.make_opaque();
                }
            }
        }
    }

    /// Which opaque helper functions this subtree needs emitted.
    pub fn helpers(&self, out: &mut BTreeSet<Helper>) {
        match self {
            Self::IntLit {
                width,
                opaque: true,
                ..
            } => {
                out.insert(if width.is_signed() {
                    Helper::I64
                } else {
                    Helper::U64
                });
            }
            Self::FloatLit {
                width,
                opaque: true,
                ..
            } => {
                out.insert(match width {
                    FloatWidth::F32 => Helper::F32,
                    FloatWidth::F64 => Helper::F64,
                });
            }
            Self::CharLit { opaque: true, .. } => {
                out.insert(Helper::Char);
            }
            Self::BoolLit { opaque: true, .. }
            | Self::BareInt { opaque: true, .. }
            | Self::BareFloat { opaque: true, .. }
            | Self::ConstRef { opaque: true, .. } => {
                out.insert(Helper::True);
            }
            Self::Block { stmts, tail } => {
                for stmt in stmts {
                    stmt.helpers(out);
                }
                tail.helpers(out);
            }
            _ => {
                for child in self.children() {
                    child.helpers(out);
                }
            }
        }
    }

    pub fn features(&self, out: &mut BTreeSet<&'static str>) {
        out.insert(self.ty().feature());
        let own = match self {
            Self::Bin { op, .. } => Some(op.feature()),
            Self::Unary { .. } => Some("lang-unary"),
            Self::Cast { .. } => Some("lang-cast"),
            Self::Call { method, .. } => {
                out.insert("lang-call");
                lookup(method).map(|entry| entry.name)
            }
            Self::If { .. } => Some("lang-if"),
            Self::FnCall { .. } => Some("lang-fn-call"),
            Self::ClosureCall { .. } => Some("lang-closure-call"),
            Self::BareInt { .. } => Some("lang-bare-int"),
            Self::BareFloat { .. } => Some("lang-bare-float"),
            Self::ConstRef { .. } => Some("lang-const"),
            Self::TupleLit(_) => Some("lang-tuple-lit"),
            Self::ResLit { .. } => Some("lang-result-lit"),
            Self::StdErrLit(_) => Some("lang-stderr-lit"),
            Self::StructLit { update: true, .. } => Some("lang-struct-update"),
            Self::StructLit { .. } => Some("lang-struct-lit"),
            Self::EnumLit { .. } => Some("lang-enum-lit"),
            Self::DefaultOf(_) => Some("lang-default"),
            Self::Field { .. } => Some("lang-field"),
            Self::TupleField { .. } => Some("lang-tuple-field"),
            Self::Index { .. } => Some("lang-index"),
            Self::Method {
                kind: crate::lang::user::MethodKind::Assoc,
                ..
            } => Some("lang-assoc-fn"),
            Self::Method { .. } => Some("lang-method"),
            Self::TraitCall { .. } => Some("lang-trait-call"),
            Self::ApplyCall { .. } => Some("lang-apply-call"),
            Self::Try { .. } => Some("lang-try"),
            Self::Into { bare: true, .. } => Some("lang-into"),
            Self::Into { .. } => Some("lang-from"),
            Self::Match { .. } => Some("lang-match"),
            Self::Block { .. } => Some("lang-block"),
            Self::Pipe(pipe) => {
                pipe.features(out);
                None
            }
            _ => None,
        };
        if let Some(feature) = own {
            out.insert(feature);
        }
        if let Self::Match { arms, .. } = self {
            for arm in arms {
                arm.pat.features(out);
                if arm.guard.is_some() {
                    out.insert("lang-pat-guard");
                }
            }
        }
        if let Self::Block { stmts, .. } = self {
            for stmt in stmts {
                stmt.features(out);
            }
        }
        for child in self.children() {
            child.features(out);
        }
    }

    /// Un-bare the receiver of every catalog call in the tree. A receiver has
    /// to state its own type before a method can be called on it, and a bare
    /// literal states nothing, so `(if c { 0 } else { 0 }).abs()` is rejected
    /// as an ambiguous `{integer}`. Run over the finished program so a
    /// receiver rebuilt after its own call was generated is covered too.
    pub fn fix_call_receivers(&mut self) {
        if let Self::Call { recv, .. } = self {
            let taken = std::mem::replace(
                &mut **recv,
                Self::BoolLit {
                    value: false,
                    opaque: false,
                },
            );
            **recv = crate::lang::expr::unbare_deep(taken);
        }
        for child in self.children_mut() {
            child.fix_call_receivers();
        }
    }

    /// `helper(&mut cl, arg)` holds the closure mutably while the argument
    /// runs, so any second use of that closure in the same expression is a
    /// borrow conflict the real compiler rejects. The direct call means the
    /// same thing and holds the borrow for less time, so the apply form is
    /// the one that gives way.
    pub fn repair_apply_borrows(&mut self) {
        let mut uses: BTreeMap<String, usize> = BTreeMap::new();
        for node in self.nodes() {
            let (Self::ApplyCall { closure: name, .. }
            | Self::ClosureCall { name, .. }
            | Self::Var { name, .. }) = node
            else {
                continue;
            };
            *uses.entry(name.clone()).or_default() += 1;
        }
        self.demote_shared_applies(&uses);
    }

    fn demote_shared_applies(&mut self, uses: &BTreeMap<String, usize>) {
        if let Self::ApplyCall {
            closure, arg, ty, ..
        } = self
            && uses.get(closure).copied().unwrap_or_default() > 1
        {
            *self = Self::ClosureCall {
                name: closure.clone(),
                args: vec![(**arg).clone()],
                ty: ty.clone(),
            };
        }
        for child in self.children_mut() {
            child.demote_shared_applies(uses);
        }
    }

    /// Whether the rendered expression pins a concrete numeric type for the
    /// real compiler. A bare literal leaves `{integer}` or `{float}`, and a
    /// local bound to one with no annotation stays ambiguous, so an inherent
    /// numeric method on it is rejected with E0689.
    pub fn states_concrete_ty(&self) -> bool {
        match self {
            Self::BareInt { .. } | Self::BareFloat { .. } => false,
            Self::Pipe(pipe) => pipe.states_type(),
            Self::Into { bare, .. } => !bare,
            Self::If {
                then_expr,
                else_expr,
                ..
            } => then_expr.states_concrete_ty() && else_expr.states_concrete_ty(),
            Self::Match { arms, .. } => arms.iter().all(|arm| arm.body.states_concrete_ty()),
            Self::Block { tail, .. } => tail.states_concrete_ty(),
            // A shift takes its type from the left operand alone, so a typed
            // count on the right does not rescue a bare value on the left.
            Self::Bin {
                op: BinOp::Shl | BinOp::Shr,
                left,
                ..
            } => left.states_concrete_ty(),
            // Either operand of the other operators types the whole thing.
            Self::Bin { left, right, .. } => {
                left.states_concrete_ty() || right.states_concrete_ty()
            }
            Self::Unary { value, .. } => value.states_concrete_ty(),
            _ => true,
        }
    }

    /// Whether the tree contains a call of the helper function or closure
    /// `name`.
    pub fn calls_fn(&self, name: &str) -> bool {
        match self {
            Self::FnCall { name: called, .. }
            | Self::ClosureCall { name: called, .. }
            | Self::ApplyCall { helper: called, .. }
                if called == name =>
            {
                true
            }
            _ => self.children().iter().any(|child| child.calls_fn(name)),
        }
    }

    /// Simpler expressions of the same type, tried in order by the reducer.
    pub fn shrinks(&self) -> Vec<Self> {
        let ty = self.ty();
        let mut candidates = Vec::new();
        let smallest = minimal(&ty);
        if *self != smallest {
            candidates.push(smallest);
        }
        for child in self.children() {
            if child.ty() == ty {
                candidates.push(child.clone());
            }
        }
        if let Some(shorter) = self.pop_item() {
            candidates.push(shorter);
        }
        if let Self::Pipe(pipe) = self {
            candidates.extend(pipe.shrinks().into_iter().map(|p| Self::Pipe(Box::new(p))));
        }
        if let Self::Match { arms, .. } = self
            && arms.len() > 1
        {
            candidates.extend(shrink_arms(self, arms));
        }
        let child_count = self.children().len();
        for index in 0..child_count {
            let child_shrinks: Vec<Expr> = self.children()[index]
                .shrinks()
                .into_iter()
                .take(CHILD_SHRINKS)
                .collect();
            for shrunk in child_shrinks {
                let mut candidate = self.clone();
                if let Some(slot) = candidate.children_mut().into_iter().nth(index) {
                    *slot = shrunk;
                }
                candidates.push(candidate);
            }
        }
        candidates
    }

    /// A literal collection one item shorter.
    fn pop_item(&self) -> Option<Self> {
        let mut shorter = self.clone();
        match &mut shorter {
            Self::VecLit { items, .. } | Self::SetLit { items, .. } if !items.is_empty() => {
                items.pop();
            }
            Self::MapLit { items, .. } if !items.is_empty() => {
                items.pop();
            }
            _ => return None,
        }
        Some(shorter)
    }
}

/// A match with one arm dropped, when what remains still covers every
/// value: only arms before an irrefutable final arm can go.
fn shrink_arms(whole: &Expr, arms: &[Arm]) -> Vec<Expr> {
    let mut out = Vec::new();
    let last_covers = arms
        .last()
        .is_some_and(|arm| arm.pat.is_irrefutable() && arm.guard.is_none());
    if !last_covers {
        return out;
    }
    for index in 0..arms.len() - 1 {
        let mut candidate = whole.clone();
        if let Expr::Match { arms, .. } = &mut candidate {
            arms.remove(index);
        }
        out.push(candidate);
    }
    out
}
