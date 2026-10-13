//! What ends a pipe and makes its value.

use std::iter::once;

use serde::{Deserialize, Serialize};

use crate::lang::expr::Expr;
use crate::lang::pipe::source::{Caps, Item};
use crate::lang::pipe::stage::Bind;
use crate::lang::ref_param::RefParam;
use crate::lang::ty::Ty;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Site {
    /// `::<T>()` on the call itself
    Turbofish,
    /// a bare call typed by the `let` annotation or the helper return type
    Bare,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Term {
    /// Into a `Vec`, a set or a map, a `String` from `char` or `String` items, or a
    /// `Result<Vec<T>, E>` from `Result<T, E>` items, which stops at the first `Err`.
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
    /// `.fold(init, |acc, item| body)`, the accumulator may be a `(a, b)` pattern
    Fold {
        acc: Bind,
        bind: Bind,
        init: Expr,
        body: Expr,
    },
    /// `.find(|r| pred)`, the closure is handed a reference like `filter`
    Find {
        bind: Bind,
        pred: Expr,
        by: RefParam,
    },
    /// `.find_map(|item| body)`, the body makes an `Option`
    FindMap {
        bind: Bind,
        body: Expr,
    },
    /// `.max_by_key(|r| key)`. The last of equal keys wins, the first for `min_by_key`.
    MaxByKey {
        bind: Bind,
        key: Expr,
        by: RefParam,
    },
    MinByKey {
        bind: Bind,
        key: Expr,
        by: RefParam,
    },
    /// `.partition(|r| pred)` into a pair of vecs
    Partition {
        bind: Bind,
        pred: Expr,
        by: RefParam,
    },
    /// `.unzip()` of pair items into a pair of vecs
    Unzip,
    /// `.for_each(|item| out.push(body))` in a block that hands the vec out
    ForEach {
        bind: Bind,
        body: Expr,
    },
    /// `.reduce(|acc, item| body)`
    Reduce {
        acc: Bind,
        bind: Bind,
        body: Expr,
    },
    /// `.rposition(|item| pred)`, which calls the closure from the back
    Rposition {
        bind: Bind,
        pred: Expr,
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

    pub fn ty(&self, item: &Item) -> Ty {
        let pair_of_vecs =
            |first: Ty, second: Ty| Ty::Tuple(vec![Ty::vec_of(first), Ty::vec_of(second)]);
        match self {
            Self::Collect { target, .. } => target.clone(),
            Self::Sum { out, .. } | Self::Product { out, .. } => out.clone(),
            Self::Count => Ty::USIZE,
            Self::Min
            | Self::Max
            | Self::Last
            | Self::Nth(_)
            | Self::Find { .. }
            | Self::MaxByKey { .. }
            | Self::MinByKey { .. }
            | Self::Reduce { .. } => Ty::opt_of(item.ty()),
            Self::Any { .. } | Self::All { .. } => Ty::Bool,
            Self::Position { .. } | Self::Rposition { .. } => Ty::opt_of(Ty::USIZE),
            Self::Fold { init, .. } => init.ty(),
            Self::FindMap { body, .. } => body.ty(),
            Self::Partition { .. } => pair_of_vecs(item.ty(), item.ty()),
            Self::Unzip => match item.halves() {
                Some((first, second)) => pair_of_vecs(first, second),
                None => pair_of_vecs(item.ty(), item.ty()),
            },
            Self::ForEach { body, .. } => Ty::vec_of(body.ty()),
        }
    }

    /// Whether `rustc` takes the terminal on a chain with these items and capabilities.
    pub fn fits(&self, item: &Item, caps: Caps) -> bool {
        match self {
            Self::Rposition { .. } => caps.double_ended && caps.exact_size,
            Self::Unzip => item.halves().is_some(),
            Self::Collect {
                target: Ty::Str, ..
            } => matches!(item, Item::Scalar(Ty::Char | Ty::Str)),
            Self::Collect {
                target: Ty::Res(ok, err),
                ..
            } => match (&**ok, item) {
                (Ty::Vec(elem), Item::Scalar(Ty::Res(item_ok, item_err))) => {
                    elem == item_ok && err == item_err
                }
                _ => false,
            },
            _ => true,
        }
    }

    pub(super) fn order_sensitive(&self, item: &Item) -> bool {
        match self {
            // a map or set forgets arrival order, the other targets keep it
            Self::Collect { target, .. } => matches!(target, Ty::Vec(_) | Ty::Str | Ty::Res(..)),
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
            | Self::Fold { .. }
            | Self::Find { .. }
            | Self::FindMap { .. }
            | Self::MaxByKey { .. }
            | Self::MinByKey { .. }
            | Self::Partition { .. }
            | Self::Unzip
            | Self::ForEach { .. }
            | Self::Reduce { .. }
            | Self::Rposition { .. } => true,
        }
    }

    /// The same arrival order leak as `Stage::leaks_order`.
    pub(super) fn leaks_order(&self, item: &Item) -> bool {
        match self {
            Self::Any { pred, .. } | Self::All { pred, .. } | Self::Position { pred, .. } => {
                pred.has_fallible_op() || item.contains_trace()
            }
            Self::Fold { init, body, .. } => {
                init.has_fallible_op()
                    || body.has_fallible_op()
                    || item.contains_trace()
                    || init.ty().contains_trace()
            }
            Self::Collect { .. }
            | Self::Sum { .. }
            | Self::Product { .. }
            | Self::Count
            | Self::Min
            | Self::Max
            | Self::Last
            | Self::Nth(_) => false,
            // each of these is order sensitive on its own already
            Self::Find { .. }
            | Self::FindMap { .. }
            | Self::MaxByKey { .. }
            | Self::MinByKey { .. }
            | Self::Partition { .. }
            | Self::Unzip
            | Self::ForEach { .. }
            | Self::Reduce { .. }
            | Self::Rposition { .. } => true,
        }
    }

    /// The bodies that live in the closure of the terminal.
    pub fn closure_exprs(&self) -> Vec<&Expr> {
        match self {
            Self::Any { pred, .. }
            | Self::All { pred, .. }
            | Self::Position { pred, .. }
            | Self::Find { pred, .. }
            | Self::Partition { pred, .. }
            | Self::Rposition { pred, .. } => vec![pred],
            Self::Fold { body, .. }
            | Self::FindMap { body, .. }
            | Self::ForEach { body, .. }
            | Self::Reduce { body, .. } => vec![body],
            Self::MaxByKey { key, .. } | Self::MinByKey { key, .. } => vec![key],
            _ => Vec::new(),
        }
    }

    /// Every expression of the terminal, in the order it is written.
    pub fn exprs(&self) -> Vec<&Expr> {
        match self {
            Self::Fold { init, body, .. } => vec![init, body],
            _ => self.closure_exprs(),
        }
    }

    pub fn exprs_mut(&mut self) -> Vec<&mut Expr> {
        match self {
            Self::Any { pred, .. }
            | Self::All { pred, .. }
            | Self::Position { pred, .. }
            | Self::Find { pred, .. }
            | Self::Partition { pred, .. }
            | Self::Rposition { pred, .. } => vec![pred],
            Self::Fold { init, body, .. } => vec![init, body],
            Self::FindMap { body, .. } | Self::ForEach { body, .. } | Self::Reduce { body, .. } => {
                vec![body]
            }
            Self::MaxByKey { key, .. } | Self::MinByKey { key, .. } => vec![key],
            _ => Vec::new(),
        }
    }

    /// The names the closure of the terminal binds.
    pub fn binds(&self) -> Vec<&Bind> {
        match self {
            Self::Any { bind, .. }
            | Self::All { bind, .. }
            | Self::Position { bind, .. }
            | Self::Find { bind, .. }
            | Self::FindMap { bind, .. }
            | Self::MaxByKey { bind, .. }
            | Self::MinByKey { bind, .. }
            | Self::Partition { bind, .. }
            | Self::ForEach { bind, .. }
            | Self::Rposition { bind, .. } => vec![bind],
            Self::Fold { acc, bind, .. } | Self::Reduce { acc, bind, .. } => vec![acc, bind],
            _ => Vec::new(),
        }
    }

    /// How a closure that is handed a reference names its item, when the terminal has one.
    pub(super) fn ref_param(&self) -> Option<RefParam> {
        match self {
            Self::Find { by, .. }
            | Self::MaxByKey { by, .. }
            | Self::MinByKey { by, .. }
            | Self::Partition { by, .. } => Some(*by),
            _ => None,
        }
    }

    pub(super) fn features(&self) -> Vec<&'static str> {
        let own = match self {
            Self::Collect {
                site: Site::Turbofish,
                ..
            } => "lang-pipe-collect-fish",
            Self::Collect {
                site: Site::Bare, ..
            } => "lang-pipe-collect-bare",
            Self::Sum {
                site: Site::Turbofish,
                ..
            } => "lang-pipe-sum",
            Self::Sum {
                site: Site::Bare, ..
            } => "lang-pipe-sum-bare",
            Self::Product { .. } => "lang-pipe-product",
            Self::Count => "lang-pipe-count",
            Self::Min | Self::Max => "lang-pipe-minmax",
            Self::Last => "lang-pipe-last",
            Self::Nth(_) => "lang-pipe-nth",
            Self::Any { .. } => "lang-pipe-any",
            Self::All { .. } => "lang-pipe-all",
            Self::Position { .. } => "lang-pipe-position",
            Self::Fold { .. } => "lang-pipe-fold",
            Self::Find { .. } => "lang-pipe-find",
            Self::FindMap { .. } => "lang-pipe-find-map",
            Self::MaxByKey { .. } => "lang-pipe-max-by-key",
            Self::MinByKey { .. } => "lang-pipe-min-by-key",
            Self::Partition { .. } => "lang-pipe-partition",
            Self::Unzip => "lang-pipe-unzip",
            Self::ForEach { .. } => "lang-pipe-for-each",
            Self::Reduce { .. } => "lang-pipe-reduce",
            Self::Rposition { .. } => "lang-pipe-rposition",
        };
        let target = match self {
            Self::Collect {
                target: Ty::Str, ..
            } => Some("lang-pipe-collect-string"),
            Self::Collect {
                target: Ty::Res(..),
                ..
            } => Some("lang-pipe-collect-result"),
            _ => None,
        };
        once(own).chain(target).collect()
    }

    /// The whole pipe, the terminal put after `chain`.
    pub(super) fn render_onto(&self, chain: &str, item: &Item) -> String {
        let ty = item.rust();
        let by_value =
            |bind: &Bind, body: &Expr| format!("|{}: {ty}| {}", bind.pattern(), body.render());
        let by_ref = |bind: &Bind, by: RefParam, body: &Expr| {
            by.closure(&bind.pattern(), Some(&ty), &body.render())
        };
        let cloned = |bind: &Bind, pred: &Expr| {
            format!(
                "|diff_ref| {{ let {}: {ty} = diff_ref.clone(); {} }}",
                bind.pattern(),
                pred.render()
            )
        };
        let call = match self {
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
            Self::Any { bind, pred } => format!(".any({})", cloned(bind, pred)),
            Self::All { bind, pred } => format!(".all({})", cloned(bind, pred)),
            Self::Position { bind, pred } => format!(".position({})", cloned(bind, pred)),
            Self::Fold {
                acc,
                bind,
                init,
                body,
            } => format!(
                ".fold({}, |{}, {}| {})",
                init.render(),
                acc.pattern(),
                bind.pattern(),
                body.render()
            ),
            Self::Find { bind, pred, by } => format!(".find({})", by_ref(bind, *by, pred)),
            Self::FindMap { bind, body } => format!(".find_map({})", by_value(bind, body)),
            Self::MaxByKey { bind, key, by } => {
                format!(".max_by_key({})", by_ref(bind, *by, key))
            }
            Self::MinByKey { bind, key, by } => {
                format!(".min_by_key({})", by_ref(bind, *by, key))
            }
            Self::Partition { bind, pred, by } => {
                format!(".partition::<Vec<{ty}>, _>({})", by_ref(bind, *by, pred))
            }
            Self::Unzip => match item.halves() {
                Some((first, second)) => {
                    let (first, second) = (first.rust(), second.rust());
                    format!(".unzip::<{first}, {second}, Vec<{first}>, Vec<{second}>>()")
                }
                None => String::new(),
            },
            Self::ForEach { bind, body } => {
                return format!(
                    "{{ let mut diff_out: Vec<{}> = Vec::new(); {chain}.for_each(|{}: {ty}| diff_out.push({})); diff_out }}",
                    body.ty().rust(),
                    bind.pattern(),
                    body.render()
                );
            }
            Self::Reduce { acc, bind, body } => format!(
                ".reduce(|{}: {ty}, {}: {ty}| {})",
                acc.pattern(),
                bind.pattern(),
                body.render()
            ),
            Self::Rposition { bind, pred } => format!(".rposition({})", by_value(bind, pred)),
        };
        format!("{chain}{call}")
    }
}
