//! Iterator pipelines: a source, a chain of adapters, and a terminal.
//!
//! This is the shape the older families could never emit, and the shape where
//! the collect-target inference in the interpreter lives. The terminal's
//! `collect` states its target at one of the real annotation sites: a
//! turbofish on the call, the annotation of the `let` that binds the result,
//! or the return type of a generated helper function.
//!
//! Determinism rule: a map or set source iterates in an order real Rust
//! randomizes per process. Such a pipeline either passes through a `Sorted`
//! stage before anything order-sensitive, or ends in a terminal whose result
//! cannot depend on order. Float items are the same problem one level down,
//! float addition is not associative, so float-typed items are only allowed
//! on ordered pipelines. `assert_deterministic` re-checks the invariant on
//! every generated and every shrunk pipe.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::lang::expr::{Expr, Helper};
use crate::lang::ty::Ty;

/// The type of one item flowing between stages.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Item {
    Scalar(Ty),
    Pair(Ty, Ty),
}

impl Item {
    pub fn rust(&self) -> String {
        match self {
            Self::Scalar(ty) => ty.rust(),
            Self::Pair(key, value) => format!("({}, {})", key.rust(), value.rust()),
        }
    }

    pub fn is_ord(&self) -> bool {
        match self {
            Self::Scalar(ty) => ty_is_ord(ty),
            Self::Pair(key, value) => ty_is_ord(key) && ty_is_ord(value),
        }
    }

    fn is_float(&self) -> bool {
        match self {
            Self::Scalar(ty) => matches!(ty, Ty::Float(_)),
            Self::Pair(..) => false,
        }
    }
}

/// Whether `sort`, `min`, and `max` compile for values of this type. Floats
/// have no total order, maps and sets implement no order at all.
pub fn ty_is_ord(ty: &Ty) -> bool {
    match ty {
        Ty::Float(_) | Ty::Map(..) | Ty::Set(_) => false,
        Ty::Vec(inner) | Ty::Opt(inner) => ty_is_ord(inner),
        _ => true,
    }
}

/// How a collection expression becomes an iterator.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Access {
    /// `vec.into_iter()`, ordered scalar items.
    VecInto,
    /// `set.into_iter()`, unordered scalar items.
    SetInto,
    /// `map.into_iter()`, unordered pair items.
    MapPairs,
    /// `map.into_keys()`, unordered key items.
    MapKeys,
    /// `map.into_values()`, unordered value items.
    MapValues,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Source {
    Coll {
        expr: Expr,
        access: Access,
    },
    /// `(start..start + count)`, ordered i64 items.
    Range {
        start: i64,
        count: u8,
    },
}

impl Source {
    pub fn item(&self) -> Item {
        match self {
            Self::Range { .. } => Item::Scalar(Ty::I64),
            Self::Coll { expr, access } => {
                let ty = expr.ty();
                match access {
                    Access::VecInto | Access::SetInto => {
                        Item::Scalar(ty.elem().cloned().unwrap_or(Ty::I64))
                    }
                    Access::MapPairs => match ty.key_val() {
                        Some((key, value)) => Item::Pair(key.clone(), value.clone()),
                        None => Item::Scalar(Ty::I64),
                    },
                    Access::MapKeys => match ty.key_val() {
                        Some((key, _)) => Item::Scalar(key.clone()),
                        None => Item::Scalar(Ty::I64),
                    },
                    Access::MapValues => match ty.key_val() {
                        Some((_, value)) => Item::Scalar(value.clone()),
                        None => Item::Scalar(Ty::I64),
                    },
                }
            }
        }
    }

    pub fn ordered(&self) -> bool {
        match self {
            Self::Range { .. } => true,
            Self::Coll { access, .. } => matches!(access, Access::VecInto),
        }
    }

    fn render(&self) -> String {
        match self {
            Self::Range { start, count } => {
                let end = start.saturating_add(i64::from(*count));
                format!("({start}i64..{end}i64)")
            }
            Self::Coll { expr, access } => {
                let call = match access {
                    Access::VecInto | Access::SetInto | Access::MapPairs => "into_iter",
                    Access::MapKeys => "into_keys",
                    Access::MapValues => "into_values",
                };
                format!("{}.{call}()", expr.render())
            }
        }
    }
}

/// A closure binding, one name for scalar items, two for pair items. The
/// names carry a per-pipe id so nesting never shadows.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Bind {
    One(String),
    Pair(String, String),
}

impl Bind {
    /// The owned-pattern form: `x` or `(k, v)`.
    fn pattern(&self) -> String {
        match self {
            Self::One(name) => name.clone(),
            Self::Pair(key, value) => format!("({key}, {value})"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Stage {
    /// `.map(|item| body)`, item to `Scalar(body.ty())`.
    Map {
        bind: Bind,
        body: Expr,
    },
    /// `.map(|k| (k.clone(), body))`, `Scalar(k)` to `Pair(k, body.ty())`.
    PairWith {
        bind: Bind,
        body: Expr,
    },
    /// `.filter(|r| { let item = r.clone(); pred })`, type preserving.
    Filter {
        bind: Bind,
        pred: Expr,
    },
    /// Order sensitive, ordered pipelines only.
    Rev,
    Take(u8),
    Skip(u8),
    /// `.enumerate()` with the index widened to i64, `Scalar(t)` to
    /// `Pair(i64, t)`. Order sensitive.
    Enumerate,
    /// Materialize, sort, re-iterate. Makes an unordered pipeline ordered,
    /// which is the only door from a map or set source to an order-sensitive
    /// stage or terminal.
    Sorted,
}

impl Stage {
    /// The item type after this stage, given the one before it.
    fn out(&self, item: &Item) -> Item {
        match self {
            Self::Map { body, .. } => Item::Scalar(body.ty()),
            Self::PairWith { body, .. } => match item {
                Item::Scalar(key) => Item::Pair(key.clone(), body.ty()),
                Item::Pair(..) => item.clone(),
            },
            Self::Enumerate => match item {
                Item::Scalar(ty) => Item::Pair(Ty::I64, ty.clone()),
                Item::Pair(..) => item.clone(),
            },
            Self::Filter { .. } | Self::Rev | Self::Take(_) | Self::Skip(_) | Self::Sorted => {
                item.clone()
            }
        }
    }

    /// Whether the stage's own result depends on the order items arrive in.
    fn order_sensitive(&self) -> bool {
        matches!(
            self,
            Self::Rev | Self::Take(_) | Self::Skip(_) | Self::Enumerate
        )
    }

    fn render(&self, item: &Item) -> String {
        match self {
            Self::Map { bind, body } => {
                format!(
                    ".map(|{}: {}| {})",
                    bind.pattern(),
                    item.rust(),
                    body.render()
                )
            }
            Self::PairWith { bind, body } => {
                let name = match bind {
                    Bind::One(name) => name.clone(),
                    Bind::Pair(key, _) => key.clone(),
                };
                format!(
                    ".map(|{name}: {}| ({name}.clone(), {}))",
                    item.rust(),
                    body.render()
                )
            }
            Self::Filter { bind, pred } => {
                let (pattern, own) = filter_clone(bind, item);
                format!(
                    ".filter(|diff_ref| {{ let {pattern}: {own} = diff_ref.clone(); {} }})",
                    pred.render()
                )
            }
            Self::Rev => ".rev()".to_string(),
            Self::Take(count) => format!(".take({count}usize)"),
            Self::Skip(count) => format!(".skip({count}usize)"),
            Self::Enumerate => match item {
                Item::Scalar(ty) => format!(
                    ".enumerate().map(|(diff_i, diff_x): (usize, {})| ((diff_i as i64), diff_x))",
                    ty.rust()
                ),
                Item::Pair(..) => String::new(),
            },
            Self::Sorted => String::new(),
        }
    }
}

/// The pattern and type a filter closure clones its borrowed item into.
fn filter_clone(bind: &Bind, item: &Item) -> (String, String) {
    (bind.pattern(), item.rust())
}

/// Where a `collect` states its target type.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Site {
    /// `collect::<T>()` on the call itself.
    Turbofish,
    /// Bare `collect()`, the type stated by the context: the annotation of
    /// the `let` that binds the pipe, or the return type of the helper
    /// function whose body the pipe is.
    Bare,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Term {
    Collect {
        target: Ty,
        site: Site,
    },
    /// `.sum::<T>()` with `T` the item type, so a u8 sum panics at the u8
    /// bound exactly like debug Rust.
    Sum {
        out: Ty,
    },
    Count,
    Min,
    Max,
    Any {
        bind: Bind,
        pred: Expr,
    },
    Position {
        bind: Bind,
        pred: Expr,
    },
    Fold {
        acc: String,
        bind: Bind,
        init: Expr,
        body: Expr,
    },
}

impl Term {
    fn order_sensitive(&self, item: &Item) -> bool {
        match self {
            // Collecting into a map or set forgets arrival order, a vec keeps it.
            Self::Collect { target, .. } => matches!(target, Ty::Vec(_)),
            Self::Sum { .. } => item.is_float(),
            Self::Count | Self::Min | Self::Max | Self::Any { .. } => false,
            Self::Position { .. } | Self::Fold { .. } => true,
        }
    }

    fn render(&self, item: &Item) -> String {
        match self {
            Self::Collect {
                target,
                site: Site::Turbofish,
            } => format!(".collect::<{}>()", target.rust()),
            Self::Collect {
                site: Site::Bare, ..
            } => ".collect()".to_string(),
            Self::Sum { out } => format!(".sum::<{}>()", out.rust()),
            Self::Count => ".count()".to_string(),
            Self::Min => ".min()".to_string(),
            Self::Max => ".max()".to_string(),
            Self::Any { bind, pred } => {
                let (pattern, own) = filter_clone(bind, item);
                format!(
                    ".any(|diff_ref| {{ let {pattern}: {own} = diff_ref.clone(); {} }})",
                    pred.render()
                )
            }
            Self::Position { bind, pred } => {
                let (pattern, own) = filter_clone(bind, item);
                format!(
                    ".position(|diff_ref| {{ let {pattern}: {own} = diff_ref.clone(); {} }})",
                    pred.render()
                )
            }
            Self::Fold {
                acc,
                bind,
                init,
                body,
            } => format!(
                ".fold({}, |{acc}, {}| {})",
                init.render(),
                bind.pattern(),
                body.render()
            ),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Pipe {
    pub source: Source,
    pub stages: Vec<Stage>,
    pub term: Term,
}

impl Pipe {
    /// The item type arriving at the terminal.
    pub fn final_item(&self) -> Item {
        let mut item = self.source.item();
        for stage in &self.stages {
            item = stage.out(&item);
        }
        item
    }

    pub fn ty(&self) -> Ty {
        let item = self.final_item();
        match &self.term {
            Term::Collect { target, .. } => target.clone(),
            Term::Sum { out } => out.clone(),
            Term::Count => Ty::USIZE,
            Term::Min | Term::Max => match item {
                Item::Scalar(ty) => Ty::opt_of(ty),
                Item::Pair(..) => Ty::opt_of(Ty::I64),
            },
            Term::Any { .. } => Ty::Bool,
            Term::Position { .. } => Ty::opt_of(Ty::USIZE),
            Term::Fold { init, .. } => init.ty(),
        }
    }

    /// True when every order-sensitive point is preceded by defined order.
    /// Generation upholds this by construction, shrinking re-checks it, and a
    /// pipe that fails it is a harness bug, never a case worth keeping.
    pub fn is_deterministic(&self) -> bool {
        let mut ordered = self.source.ordered();
        let mut item = self.source.item();
        for stage in &self.stages {
            if matches!(stage, Stage::Sorted) {
                if !item.is_ord() {
                    return false;
                }
                ordered = true;
            }
            if !ordered && stage.order_sensitive() {
                return false;
            }
            if matches!(stage, Stage::Map { .. }) {
                // A fresh mapped value forgets nothing about order, the items
                // themselves may now be floats only if order is defined.
                item = stage.out(&item);
                if !ordered && item.is_float() {
                    return false;
                }
                continue;
            }
            item = stage.out(&item);
        }
        if !ordered && self.term.order_sensitive(&item) {
            return false;
        }
        true
    }

    pub fn render(&self) -> String {
        let mut out = self.source.render();
        let mut item = self.source.item();
        for stage in &self.stages {
            if matches!(stage, Stage::Sorted) {
                out = format!(
                    "({{ let mut diff_sorted: Vec<{}> = {out}.collect(); diff_sorted.sort(); diff_sorted }}).into_iter()",
                    item.rust()
                );
            } else {
                out.push_str(&stage.render(&item));
            }
            item = stage.out(&item);
        }
        out.push_str(&self.term.render(&item));
        format!("({out})")
    }

    /// Every expression the pipe embeds: the source and the closure bodies.
    pub fn exprs(&self) -> Vec<&Expr> {
        let mut out = Vec::new();
        if let Source::Coll { expr, .. } = &self.source {
            out.push(expr);
        }
        for stage in &self.stages {
            match stage {
                Stage::Map { body, .. } | Stage::PairWith { body, .. } => out.push(body),
                Stage::Filter { pred, .. } => out.push(pred),
                _ => {}
            }
        }
        match &self.term {
            Term::Any { pred, .. } | Term::Position { pred, .. } => out.push(pred),
            Term::Fold { init, body, .. } => {
                out.push(init);
                out.push(body);
            }
            _ => {}
        }
        out
    }

    pub fn exprs_mut(&mut self) -> Vec<&mut Expr> {
        let mut out = Vec::new();
        if let Source::Coll { expr, .. } = &mut self.source {
            out.push(expr);
        }
        for stage in &mut self.stages {
            match stage {
                Stage::Map { body, .. } | Stage::PairWith { body, .. } => out.push(body),
                Stage::Filter { pred, .. } => out.push(pred),
                _ => {}
            }
        }
        match &mut self.term {
            Term::Any { pred, .. } | Term::Position { pred, .. } => out.push(pred),
            Term::Fold { init, body, .. } => {
                out.push(init);
                out.push(body);
            }
            _ => {}
        }
        out
    }

    pub fn uses_any(&self, names: &BTreeSet<String>) -> bool {
        self.exprs().iter().any(|expr| expr.uses_any(names))
    }

    pub fn helpers(&self, out: &mut BTreeSet<Helper>) {
        for expr in self.exprs() {
            expr.helpers(out);
        }
    }

    pub fn make_opaque(&mut self) {
        for expr in self.exprs_mut() {
            expr.make_opaque();
        }
    }

    pub fn features(&self, out: &mut BTreeSet<&'static str>) {
        out.insert("lang-pipe");
        out.insert(match &self.term {
            Term::Collect {
                site: Site::Turbofish,
                ..
            } => "lang-pipe-collect-fish",
            Term::Collect {
                site: Site::Bare, ..
            } => "lang-pipe-collect-bare",
            Term::Sum { .. } => "lang-pipe-sum",
            Term::Count => "lang-pipe-count",
            Term::Min | Term::Max => "lang-pipe-minmax",
            Term::Any { .. } => "lang-pipe-any",
            Term::Position { .. } => "lang-pipe-position",
            Term::Fold { .. } => "lang-pipe-fold",
        });
        for expr in self.exprs() {
            expr.features(out);
        }
    }

    /// Simpler pipes with the same result type and the invariant intact.
    pub fn shrinks(&self) -> Vec<Pipe> {
        let mut out = Vec::new();
        for index in 0..self.stages.len() {
            // Only type-preserving stages can vanish without retyping the
            // rest. `Sorted` stays, dropping it could legalize nothing and
            // break determinism.
            if matches!(
                self.stages[index],
                Stage::Filter { .. } | Stage::Rev | Stage::Take(_) | Stage::Skip(_)
            ) {
                let mut candidate = self.clone();
                candidate.stages.remove(index);
                if candidate.is_deterministic() {
                    out.push(candidate);
                }
            }
        }
        for (index, expr) in self.exprs().iter().enumerate() {
            for shrunk in expr.shrinks() {
                let mut candidate = self.clone();
                if let Some(slot) = candidate.exprs_mut().into_iter().nth(index) {
                    *slot = shrunk;
                }
                if candidate.is_deterministic() {
                    out.push(candidate);
                }
            }
        }
        out
    }
}
