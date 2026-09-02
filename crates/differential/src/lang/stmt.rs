//! Statements. Every observation is a labeled print, so a mismatch names the line that produced it.

use serde::{Deserialize, Serialize};

use crate::lang::expr::{BinOp, Expr};
use crate::lang::fmt::FmtSpec;
use crate::lang::ty::Ty;

/// An inferred binding is where the interpreter must learn a type from the initializer alone.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Ann {
    Typed,
    Inferred,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PrintForm {
    /// `{spec}`
    Plain,
    /// `{0spec}`
    Indexed,
    /// `{0spec} {0:?}`, the same argument twice
    Twice,
    /// `{0:>1$}`, the width taken from a second argument
    WidthArg(u8),
    /// `{:>diff_w$}` with `diff_w = n` named
    NamedWidth(u8),
}

impl PrintForm {
    pub(super) fn feature(self) -> &'static str {
        match self {
            Self::Plain => "lang-print",
            Self::Indexed => "lang-print-indexed",
            Self::Twice => "lang-print-twice",
            Self::WidthArg(_) => "lang-print-width-arg",
            Self::NamedWidth(_) => "lang-print-named-width",
        }
    }
}

/// One closure parameter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ClosureParam {
    /// `name: ty`
    Plain { name: String, ty: Ty },
    /// `(first, second): (A, B)`, the body reads the pieces. A pattern next to a plain parameter
    /// is where the interpreter once lost the parameter order.
    Pair {
        first: String,
        second: String,
        ty: Ty,
    },
}

impl ClosureParam {
    pub fn ty(&self) -> &Ty {
        match self {
            Self::Plain { ty, .. } | Self::Pair { ty, .. } => ty,
        }
    }

    pub(super) fn pattern(&self) -> String {
        match self {
            Self::Plain { name, ty } => format!("{name}: {}", ty.rust()),
            Self::Pair { first, second, ty } => format!("({first}, {second}): {}", ty.rust()),
        }
    }

    /// The names the body can read, with their types.
    pub fn locals(&self) -> Vec<(String, Ty)> {
        match self {
            Self::Plain { name, ty } => vec![(name.clone(), ty.clone())],
            Self::Pair { first, second, ty } => match ty {
                Ty::Tuple(parts) if parts.len() == 2 => vec![
                    (first.clone(), parts[0].clone()),
                    (second.clone(), parts[1].clone()),
                ],
                _ => Vec::new(),
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ClosureSource {
    /// `|params| -> ret { body }`, `move` when `capture_move`
    Literal {
        params: Vec<ClosureParam>,
        ret: Ty,
        body: Expr,
        capture_move: bool,
        /// the body writes a captured binding, so the closure is `FnMut`
        mutates: bool,
    },
    /// `diff_factory(arg)`, a helper returning `impl Fn(T) -> T`
    Factory { fn_name: String, arg: Expr, ty: Ty },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Stmt {
    Let {
        name: String,
        ty: Ty,
        expr: Expr,
        ann: Ann,
    },
    /// `let (a, b) = tuple;`
    LetTuple {
        names: Vec<(String, Ty)>,
        expr: Expr,
        ann: Ann,
    },
    /// A closure bound by `let`. Its calls follow at once when it borrows mutably, so the borrow
    /// ends before anything else reads the binding.
    LetClosure {
        name: String,
        source: ClosureSource,
        /// the calls printed right after the binding
        calls: Vec<Expr>,
    },
    Assign {
        name: String,
        expr: Expr,
    },
    /// `name op= expr;`
    Compound {
        name: String,
        op: BinOp,
        expr: Expr,
    },
    Print {
        label: String,
        expr: Expr,
        spec: FmtSpec,
        form: PrintForm,
    },
    If {
        condition: Expr,
        then_body: Vec<Stmt>,
        else_body: Vec<Stmt>,
    },
    ForRange {
        var: String,
        count: usize,
        body: Vec<Stmt>,
        /// `'label: for`, only when a `break` or `continue` in the body names it
        #[serde(default)]
        label: Option<String>,
    },
    /// The counter is incremented first so a `continue` can't loop forever.
    While {
        counter: String,
        limit: u8,
        body: Vec<Stmt>,
        #[serde(default)]
        label: Option<String>,
    },
    Loop {
        counter: String,
        limit: u8,
        body: Vec<Stmt>,
        #[serde(default)]
        label: Option<String>,
    },
    /// `if condition { break 'label; }`, inside a loop body only
    Break {
        condition: Expr,
        #[serde(default)]
        label: Option<String>,
    },
    /// `if condition { continue 'label; }`, inside a loop body only
    Continue {
        condition: Expr,
        #[serde(default)]
        label: Option<String>,
    },
    /// `if condition { return value; }`, inside a function body only
    Return {
        condition: Expr,
        value: Expr,
    },
    Mutate {
        name: String,
        op: MutOp,
    },
    /// `for var in source { accumulate into target }`. The source must have a defined order, so
    /// it is always a vec.
    ForAccum {
        var: String,
        source: Expr,
        target: String,
        op: MutOp,
    },
    /// `for r in name.iter_mut() { let var: T = r.clone(); *r = expr; }`
    ForMut {
        name: String,
        var: String,
        elem: Ty,
        expr: Expr,
    },
    /// `fn_name(&mut name, args);`, a helper that writes through a `&mut`
    CallMut {
        name: String,
        fn_name: String,
        args: Vec<Expr>,
    },
}

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
