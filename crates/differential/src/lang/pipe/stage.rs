//! The adapters between the source and the terminal of a pipe.

use serde::{Deserialize, Serialize};

use crate::lang::expr::Expr;
use crate::lang::own::referenced;
use crate::lang::pipe::source::{Caps, Item};
use crate::lang::ref_param::RefParam;
use crate::lang::ty::Ty;

/// The names carry a per pipe id so nesting never shadows.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Bind {
    One(String),
    Pair(String, String),
}

impl Bind {
    /// `x` or `(k, v)`
    pub(super) fn pattern(&self) -> String {
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

impl ParamAnn {
    /// The parameter of a closure that takes its item by value.
    fn param(self, bind: &Bind, item: &Item) -> String {
        match self {
            Self::Typed => format!("{}: {}", bind.pattern(), item.rust()),
            Self::Inferred => bind.pattern(),
        }
    }
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
    /// `.filter(|r| { let item = r.clone(); pred })`, type preserving. `by` picks how the
    /// closure names the item it is handed by reference.
    Filter {
        bind: Bind,
        pred: Expr,
        ann: ParamAnn,
        #[serde(default)]
        by: RefParam,
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
    /// `.filter_map(|item| body)`, the body makes an `Option` of the next item
    FilterMap {
        bind: Bind,
        body: Expr,
        ann: ParamAnn,
    },
    /// `.flat_map(|item| body)`, the body makes a `Vec` of the next items
    FlatMap {
        bind: Bind,
        body: Expr,
        ann: ParamAnn,
    },
    /// `.flatten()` over `Vec` or `Option` items
    Flatten,
    /// `.chain(other)`, a `Vec` of the item type. `other` runs when the chain is built, before
    /// any closure does.
    Chain {
        other: Expr,
    },
    /// `.zip(other)` with a `Vec`, an item `t` to `Pair(t, u)`. Order sensitive, `other` runs
    /// when the chain is built.
    Zip {
        other: Expr,
    },
    /// `.take_while(|r| pred)`, the closure is handed a reference like `filter`
    TakeWhile {
        bind: Bind,
        pred: Expr,
        #[serde(default)]
        by: RefParam,
    },
    SkipWhile {
        bind: Bind,
        pred: Expr,
        #[serde(default)]
        by: RefParam,
    },
    /// `.inspect(|x| println!(..))`, so the order the items pass in is output
    Inspect,
    /// `.scan(init, |acc, item| { *acc = body; Some(acc.clone()) })`. The body reads the state
    /// through `(*acc)`, and `init` runs when the chain is built.
    Scan {
        acc: Bind,
        bind: Bind,
        init: Expr,
        body: Expr,
    },
    Peekable,
    /// The chain goes into a `let`, `by_ref().take(n)` pulls the first items out of it, and
    /// they come back after the rest. Type preserving and order sensitive.
    ByRefTake(u8),
}

impl Stage {
    pub(crate) fn out(&self, item: &Item) -> Item {
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
            Self::FilterMap { body, .. } | Self::FlatMap { body, .. } => {
                Item::Scalar(body.ty().elem().cloned().unwrap_or(Ty::I64))
            }
            Self::Flatten => match item {
                Item::Scalar(Ty::Vec(inner) | Ty::Opt(inner)) => Item::Scalar((**inner).clone()),
                _ => item.clone(),
            },
            Self::Zip { other } => {
                Item::Pair(item.ty(), other.ty().elem().cloned().unwrap_or(Ty::I64))
            }
            Self::Scan { init, .. } => Item::Scalar(init.ty()),
            Self::Filter { .. }
            | Self::Rev
            | Self::Take(_)
            | Self::Skip(_)
            | Self::StepBy(_)
            | Self::Sorted
            | Self::Chain { .. }
            | Self::TakeWhile { .. }
            | Self::SkipWhile { .. }
            | Self::Inspect
            | Self::Peekable
            | Self::ByRefTake(_) => item.clone(),
        }
    }

    /// What the chain can do after this stage, see `Caps`.
    pub fn caps(&self, caps: Caps) -> Caps {
        match self {
            Self::Map { .. }
            | Self::PairWith { .. }
            | Self::Inspect
            | Self::Peekable
            | Self::Rev => caps,
            Self::Filter { .. }
            | Self::FilterMap { .. }
            | Self::FlatMap { .. }
            | Self::Flatten
            | Self::Chain { .. }
            | Self::ByRefTake(_) => caps.unsized_len(),
            Self::Take(_)
            | Self::Skip(_)
            | Self::StepBy(_)
            | Self::Enumerate
            | Self::Zip { .. } => caps.sized_back(),
            Self::TakeWhile { .. } | Self::SkipWhile { .. } | Self::Scan { .. } => {
                caps.forward_only()
            }
            Self::Sorted => Caps::VEC,
        }
    }

    /// Whether `rustc` takes the stage on a chain with these items and capabilities, and
    /// whether what it prints is the same on every run.
    pub fn fits(&self, item: &Item, caps: Caps) -> bool {
        match self {
            Self::Rev => caps.double_ended,
            Self::Flatten => matches!(item, Item::Scalar(Ty::Vec(_) | Ty::Opt(_))),
            Self::Enumerate | Self::PairWith { .. } => matches!(item, Item::Scalar(_)),
            // a float prints the sign of a NaN, which real Rust leaves open
            Self::Inspect => item.is_ord(),
            // the `let` ends the temporaries of its own statement
            Self::ByRefTake(_) => caps.owned,
            _ => true,
        }
    }

    pub(super) fn order_sensitive(&self) -> bool {
        matches!(
            self,
            Self::Rev
                | Self::Take(_)
                | Self::Skip(_)
                | Self::StepBy(_)
                | Self::Enumerate
                | Self::Zip { .. }
                | Self::TakeWhile { .. }
                | Self::SkipWhile { .. }
                | Self::Inspect
                | Self::Scan { .. }
                | Self::ByRefTake(_)
        )
    }

    /// A body that prints as it runs, by a panic or by a trace it drops. Its lines come out in
    /// arrival order, so it needs a defined one.
    pub(super) fn leaks_order(&self, item: &Item) -> bool {
        match self {
            Self::Map { body, .. }
            | Self::PairWith { body, .. }
            | Self::FilterMap { body, .. }
            | Self::FlatMap { body, .. } => body.has_fallible_op() || item.contains_trace(),
            Self::Filter { pred, .. }
            | Self::TakeWhile { pred, .. }
            | Self::SkipWhile { pred, .. } => pred.has_fallible_op() || item.contains_trace(),
            Self::Scan { init, body, .. } => {
                body.has_fallible_op() || item.contains_trace() || init.ty().contains_trace()
            }
            Self::Inspect => true,
            Self::Flatten | Self::Chain { .. } | Self::Zip { .. } => item.contains_trace(),
            // a zero step panics before any item flows
            Self::StepBy(step) => *step == 0,
            Self::Rev
            | Self::Take(_)
            | Self::Skip(_)
            | Self::Enumerate
            | Self::Sorted
            | Self::Peekable
            | Self::ByRefTake(_) => false,
        }
    }

    /// A body whose run shows in the output at all, a trace it makes is dropped later. Whether
    /// such a body ran is what the panic reach rule guards.
    pub(super) fn observable(&self, item: &Item) -> bool {
        self.leaks_order(item)
            || match self {
                Self::Map { body, .. }
                | Self::PairWith { body, .. }
                | Self::FilterMap { body, .. }
                | Self::FlatMap { body, .. } => body.ty().contains_trace(),
                _ => false,
            }
    }

    /// The bodies that live in a closure of the chain. A closure holds every binding it names
    /// for as long as the chain lives.
    pub fn closure_exprs(&self) -> Vec<&Expr> {
        match self {
            Self::Map { body, .. }
            | Self::PairWith { body, .. }
            | Self::FilterMap { body, .. }
            | Self::FlatMap { body, .. }
            | Self::Scan { body, .. } => vec![body],
            Self::Filter { pred, .. }
            | Self::TakeWhile { pred, .. }
            | Self::SkipWhile { pred, .. } => vec![pred],
            _ => Vec::new(),
        }
    }

    /// Every expression of the stage, in the order it is written.
    pub fn exprs(&self) -> Vec<&Expr> {
        match self {
            Self::Chain { other } | Self::Zip { other } => vec![other],
            Self::Scan { init, body, .. } => vec![init, body],
            _ => self.closure_exprs(),
        }
    }

    pub fn exprs_mut(&mut self) -> Vec<&mut Expr> {
        match self {
            Self::Map { body, .. }
            | Self::PairWith { body, .. }
            | Self::FilterMap { body, .. }
            | Self::FlatMap { body, .. } => vec![body],
            Self::Filter { pred, .. }
            | Self::TakeWhile { pred, .. }
            | Self::SkipWhile { pred, .. } => vec![pred],
            Self::Chain { other } | Self::Zip { other } => vec![other],
            Self::Scan { init, body, .. } => vec![init, body],
            _ => Vec::new(),
        }
    }

    /// The names the closures of the stage bind.
    pub fn binds(&self) -> Vec<&Bind> {
        match self {
            Self::Map { bind, .. }
            | Self::PairWith { bind, .. }
            | Self::Filter { bind, .. }
            | Self::FilterMap { bind, .. }
            | Self::FlatMap { bind, .. }
            | Self::TakeWhile { bind, .. }
            | Self::SkipWhile { bind, .. } => vec![bind],
            Self::Scan { acc, bind, .. } => vec![acc, bind],
            _ => Vec::new(),
        }
    }

    /// Whether the reducer may drop the stage. It must keep the item type, and `Sorted` stays,
    /// dropping it could break determinism.
    pub(super) fn removable(&self) -> bool {
        matches!(
            self,
            Self::Filter { .. }
                | Self::Rev
                | Self::Take(_)
                | Self::Skip(_)
                | Self::StepBy(_)
                | Self::Chain { .. }
                | Self::TakeWhile { .. }
                | Self::SkipWhile { .. }
                | Self::Inspect
                | Self::Peekable
                | Self::ByRefTake(_)
        )
    }

    pub(super) fn feature(&self) -> &'static str {
        match self {
            Self::Map {
                ann: ParamAnn::Inferred,
                ..
            }
            | Self::Filter {
                ann: ParamAnn::Inferred,
                ..
            } => "lang-pipe-param-inferred",
            Self::Map { .. } => "lang-pipe-map",
            Self::PairWith { .. } => "lang-pipe-pair",
            Self::Filter { .. } => "lang-pipe-filter",
            Self::Rev => "lang-pipe-rev",
            Self::Take(_) => "lang-pipe-take",
            Self::Skip(_) => "lang-pipe-skip",
            Self::StepBy(_) => "lang-pipe-step-by",
            Self::Enumerate => "lang-pipe-enumerate",
            Self::Sorted => "lang-pipe-sorted",
            Self::FilterMap { .. } => "lang-pipe-filter-map",
            Self::FlatMap { .. } => "lang-pipe-flat-map",
            Self::Flatten => "lang-pipe-flatten",
            Self::Chain { .. } => "lang-pipe-chain",
            Self::Zip { .. } => "lang-pipe-zip",
            Self::TakeWhile { .. } => "lang-pipe-take-while",
            Self::SkipWhile { .. } => "lang-pipe-skip-while",
            Self::Inspect => "lang-pipe-inspect",
            Self::Scan { .. } => "lang-pipe-scan",
            Self::Peekable => "lang-pipe-peekable",
            Self::ByRefTake(_) => "lang-pipe-by-ref",
        }
    }

    /// How a closure that is handed a reference names its item, when the stage has one.
    pub(super) fn ref_param(&self) -> Option<RefParam> {
        match self {
            Self::Filter { by, .. } | Self::TakeWhile { by, .. } | Self::SkipWhile { by, .. } => {
                Some(*by)
            }
            _ => None,
        }
    }

    /// The call as it follows the chain. `Sorted` and `ByRefTake` wrap the chain, see
    /// `Pipe::render`.
    pub(super) fn render(&self, item: &Item) -> String {
        let ty = item.rust();
        match self {
            Self::Map { bind, body, ann } => {
                format!(".map(|{}| {})", ann.param(bind, item), body.render())
            }
            Self::PairWith { bind, body } => {
                let name = match bind {
                    Bind::One(name) => name.clone(),
                    Bind::Pair(key, _) => key.clone(),
                };
                format!(".map(|{name}: {ty}| ({name}.clone(), {}))", body.render())
            }
            Self::Filter {
                bind,
                pred,
                ann,
                by,
            } => {
                let stated = matches!(ann, ParamAnn::Typed).then_some(ty.as_str());
                format!(
                    ".filter({})",
                    by.closure(&bind.pattern(), stated, &pred.render())
                )
            }
            Self::Rev => ".rev()".to_string(),
            Self::Take(count) => format!(".take({count}usize)"),
            Self::Skip(count) => format!(".skip({count}usize)"),
            Self::StepBy(step) => format!(".step_by({step}usize)"),
            Self::Enumerate => match item {
                Item::Scalar(_) => format!(
                    ".enumerate().map(|(diff_i, diff_x): (usize, {ty})| ((diff_i as i64), diff_x))"
                ),
                Item::Pair(..) => String::new(),
            },
            Self::Sorted | Self::ByRefTake(_) => String::new(),
            Self::FilterMap { bind, body, ann } => {
                format!(".filter_map(|{}| {})", ann.param(bind, item), body.render())
            }
            Self::FlatMap { bind, body, ann } => {
                format!(".flat_map(|{}| {})", ann.param(bind, item), body.render())
            }
            Self::Flatten => ".flatten()".to_string(),
            Self::Chain { other } => format!(".chain({})", other.render()),
            Self::Zip { other } => format!(".zip({})", other.render()),
            Self::TakeWhile { bind, pred, by } => format!(
                ".take_while({})",
                by.closure(&bind.pattern(), Some(&ty), &pred.render())
            ),
            Self::SkipWhile { bind, pred, by } => format!(
                ".skip_while({})",
                by.closure(&bind.pattern(), Some(&ty), &pred.render())
            ),
            Self::Inspect => {
                ".inspect(|diff_seen| println!(\"diff inspect {:?}\", diff_seen))".to_string()
            }
            Self::Scan {
                acc,
                bind,
                init,
                body,
            } => {
                let acc = acc.pattern();
                format!(
                    ".scan({}, |{acc}, {}: {ty}| {{ *{acc} = {}; Some((*{acc}).clone()) }})",
                    init.render(),
                    bind.pattern(),
                    body.render()
                )
            }
            Self::Peekable => ".peekable()".to_string(),
        }
    }
}

/// The bindings the closures of these stages name. They stay borrowed while the chain lives,
/// so an expression that runs when the chain is built takes none of them.
pub fn held_names(stages: &[Stage]) -> Vec<String> {
    stages
        .iter()
        .flat_map(Stage::closure_exprs)
        .flat_map(referenced)
        .collect()
}
