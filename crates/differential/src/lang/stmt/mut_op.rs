//! `MutOp`, the in place operations a mutation statement applies to a binding.

use serde::{Deserialize, Serialize};

use crate::lang::expr::Expr;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MutOp {
    VecPush(Expr),
    VecSort,
    VecDedup,
    VecReverse,
    VecPop,
    VecClear,
    VecTruncate(u8),
    VecSwap(u8, u8),
    /// `name[i] = value;`, panics out of bounds
    VecSetIndex {
        index: u8,
        value: Expr,
    },
    /// `name[i].push(value);` into a vec of vecs, panics out of bounds. Rows built with
    /// `vec![row; n]` must not share storage, so a push into one row shows in that row alone.
    VecRowPush {
        index: u8,
        value: Expr,
    },
    VecExtend(Expr),
    VecRetain {
        bind: String,
        pred: Expr,
    },
    StrPush(Expr),
    StrPushStr(Expr),
    StrClear,
    MapInsert {
        key: Expr,
        value: Expr,
    },
    MapRemove {
        key: Expr,
    },
    /// `*name.entry(key).or_insert(default) += add;`, integer values only
    MapEntryAdd {
        key: Expr,
        default: Expr,
        add: Expr,
    },
    /// `name.entry(key).or_default().push(value);`, vec values only
    MapEntryPush {
        key: Expr,
        value: Expr,
    },
    SetInsert(Expr),
    SetRemove(Expr),
    /// `name = Some(value)` through `replace`, `name.take()` observed
    OptTake,
    OptReplace(Expr),
}

impl MutOp {
    pub fn exprs(&self) -> Vec<&Expr> {
        match self {
            Self::VecPush(expr)
            | Self::VecExtend(expr)
            | Self::SetInsert(expr)
            | Self::SetRemove(expr)
            | Self::StrPush(expr)
            | Self::StrPushStr(expr)
            | Self::OptReplace(expr) => vec![expr],
            Self::VecSort
            | Self::VecDedup
            | Self::VecReverse
            | Self::VecPop
            | Self::VecClear
            | Self::VecTruncate(_)
            | Self::VecSwap(..)
            | Self::StrClear
            | Self::OptTake => Vec::new(),
            Self::VecSetIndex { value, .. } | Self::VecRowPush { value, .. } => vec![value],
            Self::VecRetain { pred, .. } => vec![pred],
            Self::MapInsert { key, value } | Self::MapEntryPush { key, value } => vec![key, value],
            Self::MapRemove { key } => vec![key],
            Self::MapEntryAdd { key, default, add } => vec![key, default, add],
        }
    }

    pub fn exprs_mut(&mut self) -> Vec<&mut Expr> {
        match self {
            Self::VecPush(expr)
            | Self::VecExtend(expr)
            | Self::SetInsert(expr)
            | Self::SetRemove(expr)
            | Self::StrPush(expr)
            | Self::StrPushStr(expr)
            | Self::OptReplace(expr) => vec![expr],
            Self::VecSort
            | Self::VecDedup
            | Self::VecReverse
            | Self::VecPop
            | Self::VecClear
            | Self::VecTruncate(_)
            | Self::VecSwap(..)
            | Self::StrClear
            | Self::OptTake => Vec::new(),
            Self::VecSetIndex { value, .. } | Self::VecRowPush { value, .. } => vec![value],
            Self::VecRetain { pred, .. } => vec![pred],
            Self::MapInsert { key, value } | Self::MapEntryPush { key, value } => vec![key, value],
            Self::MapRemove { key } => vec![key],
            Self::MapEntryAdd { key, default, add } => vec![key, default, add],
        }
    }

    pub fn render(&self, name: &str) -> String {
        match self {
            Self::VecPush(expr) | Self::StrPush(expr) => {
                format!("{name}.push({});", expr.render())
            }
            Self::VecRowPush { index, value } => {
                format!("{name}[{index}usize].push({});", value.render())
            }
            Self::VecSort => format!("{name}.sort();"),
            Self::VecDedup => format!("{name}.dedup();"),
            Self::VecReverse => format!("{name}.reverse();"),
            Self::VecPop => format!("{name}.pop();"),
            Self::VecClear | Self::StrClear => format!("{name}.clear();"),
            Self::VecTruncate(count) => format!("{name}.truncate({count}usize);"),
            Self::VecSwap(a, b) => format!("{name}.swap({a}usize, {b}usize);"),
            Self::VecSetIndex { index, value } => {
                format!("{name}[{index}usize] = {};", value.render())
            }
            Self::VecExtend(expr) => format!("{name}.extend({});", expr.render()),
            Self::VecRetain { bind, pred } => format!(
                "{name}.retain(|diff_ref| {{ let {bind} = diff_ref.clone(); {} }});",
                pred.render()
            ),
            Self::StrPushStr(expr) => format!("{name}.push_str(&{});", expr.render()),
            Self::MapInsert { key, value } => {
                format!("{name}.insert({}, {});", key.render(), value.render())
            }
            Self::MapRemove { key } => format!("{name}.remove(&{});", key.render()),
            Self::MapEntryAdd { key, default, add } => format!(
                "*{name}.entry({}).or_insert({}) += {};",
                key.render(),
                default.render(),
                add.render()
            ),
            Self::MapEntryPush { key, value } => format!(
                "{name}.entry({}).or_default().push({});",
                key.render(),
                value.render()
            ),
            Self::SetInsert(expr) => format!("{name}.insert({});", expr.render()),
            Self::SetRemove(expr) => format!("{name}.remove(&{});", expr.render()),
            Self::OptTake => format!("{name}.take();"),
            Self::OptReplace(expr) => format!("{name}.replace({});", expr.render()),
        }
    }

    /// Only entry `+=`, index writes, swap and retain bodies can abort.
    pub fn has_fallible_op(&self) -> bool {
        matches!(
            self,
            Self::MapEntryAdd { .. }
                | Self::VecSetIndex { .. }
                | Self::VecRowPush { .. }
                | Self::VecSwap(..)
        ) || self.exprs().iter().any(|expr| expr.has_fallible_op())
    }

    pub fn feature(&self) -> &'static str {
        match self {
            Self::VecPush(_) => "lang-mut-push",
            Self::VecRowPush { .. } => "lang-mut-row-push",
            Self::VecSort => "lang-mut-sort",
            Self::VecDedup => "lang-mut-dedup",
            Self::VecReverse => "lang-mut-reverse",
            Self::VecPop => "lang-mut-pop",
            Self::VecClear | Self::StrClear => "lang-mut-clear",
            Self::VecTruncate(_) => "lang-mut-truncate",
            Self::VecSwap(..) => "lang-mut-swap",
            Self::VecSetIndex { .. } => "lang-mut-index-write",
            Self::VecExtend(_) => "lang-mut-extend",
            Self::VecRetain { .. } => "lang-mut-retain",
            Self::StrPush(_) => "lang-mut-str-push",
            Self::StrPushStr(_) => "lang-mut-str-push-str",
            Self::MapInsert { .. } => "lang-mut-map-insert",
            Self::MapRemove { .. } => "lang-mut-map-remove",
            Self::MapEntryAdd { .. } => "lang-mut-entry-add",
            Self::MapEntryPush { .. } => "lang-mut-entry-push",
            Self::SetInsert(_) => "lang-mut-set-insert",
            Self::SetRemove(_) => "lang-mut-set-remove",
            Self::OptTake => "lang-mut-opt-take",
            Self::OptReplace(_) => "lang-mut-opt-replace",
        }
    }
}
