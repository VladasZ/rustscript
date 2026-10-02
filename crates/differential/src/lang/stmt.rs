//! Statements. Every observation is a labeled print, so a mismatch names the line that produced it.

use serde::{Deserialize, Serialize};

use crate::lang::expr::{BinOp, Expr};
use crate::lang::fmt::FmtSpec;
use crate::lang::pat::Pat;
use crate::lang::ref_param::RefParam;
use crate::lang::stmt_render::field_name;
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

/// One link of an `if let` chain, joined by `&&`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ChainLink {
    Cond(Expr),
    Let { pat: Pat, expr: Expr },
}

impl ChainLink {
    pub fn expr(&self) -> &Expr {
        match self {
            Self::Cond(expr) | Self::Let { expr, .. } => expr,
        }
    }

    pub fn expr_mut(&mut self) -> &mut Expr {
        match self {
            Self::Cond(expr) | Self::Let { expr, .. } => expr,
        }
    }
}

/// One arm of a `match` statement.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StmtArm {
    pub pat: Pat,
    pub guard: Option<Expr>,
    pub body: Vec<Stmt>,
}

/// How the `else` of a `let else` leaves.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Exit {
    Break(Option<String>),
    Continue(Option<String>),
    /// `return value;` in a function, a bare `return;` in `main`
    Return(Option<Expr>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Stmt {
    /// `mutable` is set by `mark_mutable` from the writes that resolve to this binding, a
    /// shadowed name may be written in one scope and not another.
    Let {
        name: String,
        ty: Ty,
        expr: Expr,
        ann: Ann,
        #[serde(default)]
        mutable: bool,
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
    /// `name.field = expr;`, which also puts a moved out field back. `base` is the type of the
    /// binding, it names the field.
    AssignField {
        name: String,
        base: Ty,
        index: usize,
        expr: Expr,
    },
    /// `std::mem::swap(&mut a, &mut b);`
    Swap {
        a: String,
        b: String,
    },
    /// `{ body }`, a scope of its own so its bindings drop at the closing brace
    Scope {
        body: Vec<Stmt>,
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
    /// `for r in name.iter_mut() { let var: T = r.clone(); write through r }`
    ForMut {
        name: String,
        var: String,
        elem: Ty,
        write: ElemWrite,
    },
    /// A `&mut` into `name`, taken by one of the forms of `MutPlace` and written through.
    /// `name` is borrowed for the whole statement, so nothing in it reads `name`. `base` is the
    /// type of the binding and `elem` the type behind the reference.
    RefMut {
        name: String,
        base: Ty,
        place: MutPlace,
        var: String,
        elem: Ty,
        write: ElemWrite,
    },
    /// `for var in &source { body }`, or one of the other forms of `ForForm`. The item is a
    /// reference, so the body reads it through `(*var)` and can only clone it. A source that
    /// is a binding is named in place and stays borrowed for the loop, any other is a
    /// temporary that lives as long as the loop.
    ForEach {
        var: String,
        elem: Ty,
        source: Expr,
        form: ForForm,
        body: Vec<Stmt>,
        #[serde(default)]
        label: Option<String>,
    },
    /// `let mut name = Vec::new(); for (k, v) in &source { name.push(item); } name.sort();`
    /// over a map or a set. Real Rust randomizes their order per process, so the body only
    /// collects and the sort after the loop makes the result one every run agrees on.
    ForUnordered {
        name: String,
        item_ty: Ty,
        /// the bindings, 2 over a map and 1 over a set, each read through `(*name)`
        binds: Vec<(String, Ty)>,
        source: Expr,
        item: Expr,
    },
    /// `fn_name(&mut name, args);`, a helper that writes through a `&mut`
    CallMut {
        name: String,
        fn_name: String,
        args: Vec<Expr>,
    },
    /// `if let pat = expr && cond && let pat = expr { .. } else { .. }`. The first link is
    /// always a `let`.
    IfLet {
        links: Vec<ChainLink>,
        then_body: Vec<Stmt>,
        else_body: Option<Vec<Stmt>>,
    },
    /// `while let pat = name.pop() { body }`. The body never sees `name`, so the vec only
    /// shrinks and the loop ends.
    WhileLet {
        name: String,
        pat: Pat,
        body: Vec<Stmt>,
        #[serde(default)]
        label: Option<String>,
    },
    /// `let pat = expr else { else_body; exit };`
    LetElse {
        pat: Pat,
        expr: Expr,
        else_body: Vec<Stmt>,
        exit: Exit,
    },
    /// `match scrutinee { pat if guard => { body } .. }`
    Match {
        scrutinee: Expr,
        /// a slice view, see `Expr::Match`
        by_ref: bool,
        arms: Vec<StmtArm>,
    },
    /// `let name: ty = loop { .. break value; };`. The counter breaks with `fallback` when
    /// `exit` never holds.
    LetLoop {
        name: String,
        ty: Ty,
        counter: String,
        limit: u8,
        body: Vec<Stmt>,
        exit: Expr,
        value: Expr,
        fallback: Expr,
    },
}

/// Where the `&mut` of a `Stmt::RefMut` points.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MutPlace {
    /// `{ let r = &mut name; .. }`, the borrow block
    Whole,
    /// `&mut name.f0`, a struct or a tuple field
    Field(usize),
    /// `&mut name[i]`, panics out of bounds
    Index(u8),
    /// `if let Some(r) = name.get_mut(i) { .. }`
    GetMut(u8),
    /// `if let Some(r) = name.last_mut() { .. }`
    LastMut,
    /// `if let Some(r) = name.get_mut(&key) { .. }` on a map
    MapGetMut(Expr),
    /// `name.entry(key).or_insert_with(|| default)`
    Entry { key: Expr, default: Expr },
    /// `for r in name.values_mut() { .. }`. The order is random per process, so the write sees
    /// its own entry alone and can neither panic nor print a drop.
    ValuesMut,
    /// `if let Some(ref mut r) = name { .. }` on an option
    OptRefMut,
}

impl MutPlace {
    /// In the order they run.
    pub fn exprs(&self) -> Vec<&Expr> {
        match self {
            Self::MapGetMut(key) => vec![key],
            Self::Entry { key, default } => vec![key, default],
            _ => Vec::new(),
        }
    }

    pub fn exprs_mut(&mut self) -> Vec<&mut Expr> {
        match self {
            Self::MapGetMut(key) => vec![key],
            Self::Entry { key, default } => vec![key, default],
            _ => Vec::new(),
        }
    }

    /// Whether the body runs only when the place is there.
    pub fn is_conditional(&self) -> bool {
        matches!(
            self,
            Self::GetMut(_) | Self::LastMut | Self::MapGetMut(_) | Self::OptRefMut
        )
    }

    /// The line that opens the statement and binds `diff_ref`.
    pub fn head(&self, name: &str, base: &Ty) -> String {
        match self {
            Self::Whole => format!("{{ let diff_ref = &mut {name};"),
            Self::Field(index) => {
                format!(
                    "{{ let diff_ref = &mut {name}.{};",
                    field_name(base, *index)
                )
            }
            Self::Index(index) => format!("{{ let diff_ref = &mut {name}[{index}usize];"),
            Self::GetMut(index) => {
                format!("if let Some(diff_ref) = {name}.get_mut({index}usize) {{")
            }
            Self::LastMut => format!("if let Some(diff_ref) = {name}.last_mut() {{"),
            Self::MapGetMut(key) => {
                format!(
                    "if let Some(diff_ref) = {name}.get_mut(&{}) {{",
                    key.render()
                )
            }
            Self::Entry { key, default } => format!(
                "{{ let diff_ref = {name}.entry({}).or_insert_with(|| {});",
                key.render(),
                default.render()
            ),
            Self::ValuesMut => format!("for diff_ref in {name}.values_mut() {{"),
            Self::OptRefMut => format!("if let Some(ref mut diff_ref) = {name} {{"),
        }
    }

    pub fn feature(&self) -> &'static str {
        match self {
            Self::Whole => "lang-ref-mut-block",
            Self::Field(_) => "lang-ref-mut-field",
            Self::Index(_) => "lang-ref-mut-index",
            Self::GetMut(_) => "lang-ref-mut-get-mut",
            Self::LastMut => "lang-ref-mut-last-mut",
            Self::MapGetMut(_) => "lang-ref-mut-map-get-mut",
            Self::Entry { .. } => "lang-ref-mut-entry",
            Self::ValuesMut => "lang-ref-mut-values-mut",
            Self::OptRefMut => "lang-ref-mut-pat",
        }
    }
}

/// How a `for` walks a vec by shared reference.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ForForm {
    /// `for x in &v`
    Ref,
    /// `for x in v.iter()`
    Iter,
    /// `for (index, x) in v.iter().enumerate()`
    Enumerate { index: String },
}

/// What an `iter_mut` loop does to each element through its `&mut`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ElemWrite {
    /// `*r = expr;`
    Assign(Expr),
    /// `*r op= expr;`
    Compound(BinOp, Expr),
    /// `r.push(expr);` and friends, the call derefs the `&mut`
    Method(Box<MutOp>),
}

impl ElemWrite {
    pub fn exprs(&self) -> Vec<&Expr> {
        match self {
            Self::Assign(expr) | Self::Compound(_, expr) => vec![expr],
            Self::Method(op) => op.exprs(),
        }
    }

    pub fn exprs_mut(&mut self) -> Vec<&mut Expr> {
        match self {
            Self::Assign(expr) | Self::Compound(_, expr) => vec![expr],
            Self::Method(op) => op.exprs_mut(),
        }
    }

    pub fn render(&self) -> String {
        match self {
            Self::Assign(expr) => format!("*diff_ref = {};", expr.render()),
            Self::Compound(op, expr) => format!("*diff_ref {}= {};", op.token(), expr.render()),
            Self::Method(op) => op.render("diff_ref"),
        }
    }

    pub fn has_fallible_op(&self) -> bool {
        match self {
            Self::Assign(_) => false,
            Self::Compound(op, _) => op.is_fallible(),
            Self::Method(op) => op.has_fallible_op(),
        }
    }

    pub fn feature(&self) -> &'static str {
        match self {
            Self::Assign(_) => "lang-iter-mut-assign",
            Self::Compound(..) => "lang-iter-mut-compound",
            Self::Method(_) => "lang-iter-mut-method",
        }
    }
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
        #[serde(default)]
        by: RefParam,
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
    /// In the order rustc evaluates them. A `+=` on an integer runs its right side before
    /// the place, so `add` comes before the `entry` key.
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
            Self::MapEntryAdd { key, default, add } => vec![add, key, default],
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
            Self::MapEntryAdd { key, default, add } => vec![add, key, default],
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
            Self::VecRetain { bind, pred, by } => {
                format!("{name}.retain({});", by.closure(bind, None, &pred.render()))
            }
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
