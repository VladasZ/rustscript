//! Iterator pipelines, a source, adapters and a terminal.
//!
//! Determinism rule. A map or set source iterates in an order real Rust randomizes, so such a pipe
//! passes a `Sorted` stage before anything order sensitive, float items and fallible closures included.
//!
//! Panic reach rule. std collects a `Vec` into a `Vec` in place and touches no item when a `Skip`
//! emptied it, so a panicking body before a `Skip` never runs there while a lazy engine runs it. A
//! `Sorted` stage runs every body before it, so it clears the flag.
//!
//! `is_valid` re-checks both rules on every generated and shrunk pipe.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::lang::expr::{Expr, Helper};
use crate::lang::ty::Ty;

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
            Self::Scalar(ty) => ty.is_ord(),
            Self::Pair(key, value) => key.is_ord() && value.is_ord(),
        }
    }

    fn is_float(&self) -> bool {
        match self {
            Self::Scalar(ty) => ty.contains_float(),
            Self::Pair(key, value) => key.contains_float() || value.contains_float(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Access {
    /// `vec.into_iter()`, ordered scalar items
    VecInto,
    /// `set.into_iter()`, unordered scalar items
    SetInto,
    /// `map.into_iter()`, unordered pair items
    MapPairs,
    /// `map.into_keys()`, unordered key items
    MapKeys,
    /// `map.into_values()`, unordered value items
    MapValues,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Source {
    Coll {
        expr: Expr,
        access: Access,
    },
    /// `(start..start + count)`, ordered i64 items
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

/// The names carry a per pipe id so nesting never shadows.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Bind {
    One(String),
    Pair(String, String),
}

impl Bind {
    /// `x` or `(k, v)`
    fn pattern(&self) -> String {
        match self {
            Self::One(name) => name.clone(),
            Self::Pair(key, value) => format!("({key}, {value})"),
        }
    }
}

/// An untyped parameter is where the interpreter must learn the item type from the chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ParamAnn {
    Typed,
    Inferred,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Stage {
    /// `.map(|item| body)`, item to `Scalar(body.ty())`
    Map {
        bind: Bind,
        body: Expr,
        ann: ParamAnn,
    },
    /// `.map(|k| (k.clone(), body))`, `Scalar(k)` to `Pair(k, body.ty())`
    PairWith {
        bind: Bind,
        body: Expr,
    },
    /// `.filter(|r| { let item = r.clone(); pred })`, type preserving
    Filter {
        bind: Bind,
        pred: Expr,
        ann: ParamAnn,
    },
    /// order sensitive, ordered pipelines only
    Rev,
    Take(u8),
    Skip(u8),
    /// `.step_by(n)`, order sensitive, panics on zero
    StepBy(u8),
    /// `.enumerate()` with the index widened to i64, `Scalar(t)` to `Pair(i64, t)`. Order sensitive.
    Enumerate,
    /// Collect, sort, re-iterate. The only door from a map or set source to an order sensitive stage.
    Sorted,
}

impl Stage {
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
            Self::Filter { .. }
            | Self::Rev
            | Self::Take(_)
            | Self::Skip(_)
            | Self::StepBy(_)
            | Self::Sorted => item.clone(),
        }
    }

    fn order_sensitive(&self) -> bool {
        matches!(
            self,
            Self::Rev | Self::Take(_) | Self::Skip(_) | Self::StepBy(_) | Self::Enumerate
        )
    }

    /// A panicking body observes arrival order, the first item to panic decides the message.
    fn fallible(&self) -> bool {
        match self {
            Self::Map { body, .. } | Self::PairWith { body, .. } => body.has_fallible_op(),
            Self::Filter { pred, .. } => pred.has_fallible_op(),
            // a zero step panics before any item flows
            Self::StepBy(step) => *step == 0,
            Self::Rev | Self::Take(_) | Self::Skip(_) | Self::Enumerate | Self::Sorted => false,
        }
    }

    fn render(&self, item: &Item) -> String {
        match self {
            Self::Map { bind, body, ann } => {
                let param = match ann {
                    ParamAnn::Typed => format!("{}: {}", bind.pattern(), item.rust()),
                    ParamAnn::Inferred => bind.pattern(),
                };
                format!(".map(|{param}| {})", body.render())
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
            Self::Filter { bind, pred, ann } => {
                let own = match ann {
                    ParamAnn::Typed => format!(": {}", item.rust()),
                    ParamAnn::Inferred => String::new(),
                };
                format!(
                    ".filter(|diff_ref| {{ let {}{own} = diff_ref.clone(); {} }})",
                    bind.pattern(),
                    pred.render()
                )
            }
            Self::Rev => ".rev()".to_string(),
            Self::Take(count) => format!(".take({count}usize)"),
            Self::Skip(count) => format!(".skip({count}usize)"),
            Self::StepBy(step) => format!(".step_by({step}usize)"),
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

/// A `Sorted` runs every body before it, so it clears the flag.
fn carries_panic(pending: bool, stage: &Stage) -> bool {
    match stage {
        Stage::Sorted => false,
        _ => pending || stage.fallible(),
    }
}

/// A `Skip` appended here would hide the panic, see the panic reach rule.
pub fn fallible_pending(stages: &[Stage]) -> bool {
    stages.iter().fold(false, carries_panic)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Site {
    /// `::<T>()` on the call itself
    Turbofish,
    /// a bare call typed by the `let` annotation or the helper return type
    Bare,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Term {
    Collect {
        target: Ty,
        site: Site,
    },
    /// `.sum()` into the item type, so a u8 sum panics at the u8 bound and doesn't wrap
    Sum {
        out: Ty,
        site: Site,
    },
    Product {
        out: Ty,
        site: Site,
    },
    Count,
    Min,
    Max,
    Last,
    Nth(u8),
    Any {
        bind: Bind,
        pred: Expr,
    },
    All {
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
    /// Whether the terminal leaves its type to the context.
    pub fn is_bare(&self) -> bool {
        matches!(
            self,
            Self::Collect {
                site: Site::Bare,
                ..
            } | Self::Sum {
                site: Site::Bare,
                ..
            } | Self::Product {
                site: Site::Bare,
                ..
            }
        )
    }

    fn order_sensitive(&self, item: &Item) -> bool {
        match self {
            // a map or set forgets arrival order, a vec keeps it
            Self::Collect { target, .. } => matches!(target, Ty::Vec(_)),
            // A float sum rounds per order, a signed sum panics on an order dependent prefix, a
            // product meets its zero in some order. An unsigned sum only grows.
            Self::Sum { out, .. } => {
                item.is_float() || matches!(out, Ty::Int(width) if width.is_signed())
            }
            Self::Count | Self::Min | Self::Max | Self::Any { .. } | Self::All { .. } => false,
            Self::Product { .. }
            | Self::Last
            | Self::Nth(_)
            | Self::Position { .. }
            | Self::Fold { .. } => true,
        }
    }

    /// The same arrival order leak as `Stage::fallible`.
    fn fallible(&self) -> bool {
        match self {
            Self::Any { pred, .. } | Self::All { pred, .. } | Self::Position { pred, .. } => {
                pred.has_fallible_op()
            }
            Self::Fold { init, body, .. } => init.has_fallible_op() || body.has_fallible_op(),
            Self::Collect { .. }
            | Self::Sum { .. }
            | Self::Product { .. }
            | Self::Count
            | Self::Min
            | Self::Max
            | Self::Last
            | Self::Nth(_) => false,
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
            Self::Sum {
                out,
                site: Site::Turbofish,
            } => format!(".sum::<{}>()", out.rust()),
            Self::Sum {
                site: Site::Bare, ..
            } => ".sum()".to_string(),
            Self::Product {
                out,
                site: Site::Turbofish,
            } => format!(".product::<{}>()", out.rust()),
            Self::Product {
                site: Site::Bare, ..
            } => ".product()".to_string(),
            Self::Count => ".count()".to_string(),
            Self::Min => ".min()".to_string(),
            Self::Max => ".max()".to_string(),
            Self::Last => ".last()".to_string(),
            Self::Nth(index) => format!(".nth({index}usize)"),
            Self::Any { bind, pred } => format!(
                ".any(|diff_ref| {{ let {}: {} = diff_ref.clone(); {} }})",
                bind.pattern(),
                item.rust(),
                pred.render()
            ),
            Self::All { bind, pred } => format!(
                ".all(|diff_ref| {{ let {}: {} = diff_ref.clone(); {} }})",
                bind.pattern(),
                item.rust(),
                pred.render()
            ),
            Self::Position { bind, pred } => format!(
                ".position(|diff_ref| {{ let {}: {} = diff_ref.clone(); {} }})",
                bind.pattern(),
                item.rust(),
                pred.render()
            ),
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
            Term::Sum { out, .. } | Term::Product { out, .. } => out.clone(),
            Term::Count => Ty::USIZE,
            Term::Min | Term::Max | Term::Last | Term::Nth(_) => match item {
                Item::Scalar(ty) => Ty::opt_of(ty),
                Item::Pair(key, value) => Ty::opt_of(Ty::Tuple(vec![key, value])),
            },
            Term::Any { .. } | Term::All { .. } => Ty::Bool,
            Term::Position { .. } => Ty::opt_of(Ty::USIZE),
            Term::Fold { init, .. } => init.ty(),
        }
    }

    /// A bare `collect`, `sum` or `product` needs the context to state it.
    pub fn states_type(&self) -> bool {
        !matches!(
            self.term,
            Term::Collect {
                site: Site::Bare,
                ..
            } | Term::Sum {
                site: Site::Bare,
                ..
            } | Term::Product {
                site: Site::Bare,
                ..
            }
        )
    }

    /// Both rules from the module docs. A pipe that fails one is a harness bug.
    pub fn is_valid(&self) -> bool {
        self.is_deterministic() && self.panics_reach_output()
    }

    /// The panic reach rule.
    pub fn panics_reach_output(&self) -> bool {
        let mut pending = false;
        for stage in &self.stages {
            if matches!(stage, Stage::Skip(_)) && pending {
                return false;
            }
            pending = carries_panic(pending, stage);
        }
        true
    }

    /// The determinism rule.
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
            if !ordered && (stage.order_sensitive() || stage.fallible()) {
                return false;
            }
            if matches!(stage, Stage::Map { .. }) {
                // mapped items may be floats only if order is defined
                item = stage.out(&item);
                if !ordered && item.is_float() {
                    return false;
                }
                continue;
            }
            item = stage.out(&item);
        }
        if !ordered && (self.term.order_sensitive(&item) || self.term.fallible()) {
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

    /// The source and the closure bodies.
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
            Term::Any { pred, .. } | Term::All { pred, .. } | Term::Position { pred, .. } => {
                out.push(pred);
            }
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
            Term::Any { pred, .. } | Term::All { pred, .. } | Term::Position { pred, .. } => {
                out.push(pred);
            }
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
            Term::Sum {
                site: Site::Turbofish,
                ..
            } => "lang-pipe-sum",
            Term::Sum {
                site: Site::Bare, ..
            } => "lang-pipe-sum-bare",
            Term::Product { .. } => "lang-pipe-product",
            Term::Count => "lang-pipe-count",
            Term::Min | Term::Max => "lang-pipe-minmax",
            Term::Last => "lang-pipe-last",
            Term::Nth(_) => "lang-pipe-nth",
            Term::Any { .. } => "lang-pipe-any",
            Term::All { .. } => "lang-pipe-all",
            Term::Position { .. } => "lang-pipe-position",
            Term::Fold { .. } => "lang-pipe-fold",
        });
        for stage in &self.stages {
            out.insert(match stage {
                Stage::Map {
                    ann: ParamAnn::Inferred,
                    ..
                }
                | Stage::Filter {
                    ann: ParamAnn::Inferred,
                    ..
                } => "lang-pipe-param-inferred",
                Stage::Map { .. } => "lang-pipe-map",
                Stage::PairWith { .. } => "lang-pipe-pair",
                Stage::Filter { .. } => "lang-pipe-filter",
                Stage::Rev => "lang-pipe-rev",
                Stage::Take(_) => "lang-pipe-take",
                Stage::Skip(_) => "lang-pipe-skip",
                Stage::StepBy(_) => "lang-pipe-step-by",
                Stage::Enumerate => "lang-pipe-enumerate",
                Stage::Sorted => "lang-pipe-sorted",
            });
        }
        for expr in self.exprs() {
            expr.features(out);
        }
    }

    pub fn shrinks(&self) -> Vec<Pipe> {
        let mut out = Vec::new();
        for index in 0..self.stages.len() {
            // only type preserving stages can vanish, `Sorted` stays, dropping it could break
            // determinism
            if matches!(
                self.stages[index],
                Stage::Filter { .. }
                    | Stage::Rev
                    | Stage::Take(_)
                    | Stage::Skip(_)
                    | Stage::StepBy(_)
            ) {
                let mut candidate = self.clone();
                candidate.stages.remove(index);
                if candidate.is_valid() {
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
                if candidate.is_valid() {
                    out.push(candidate);
                }
            }
        }
        out
    }
}
