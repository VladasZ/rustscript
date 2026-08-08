//! The typed expression tree. Every node carries its own result type, so the
//! generator, the renderer and the shrinker all agree on what a subtree means
//! without re-running inference.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::lang::catalog::{METHODS, Method};
use crate::lang::pipe::Pipe;
use crate::lang::ty::Ty;
use crate::numeric::{FloatWidth, IntWidth};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    And,
    Or,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl BinOp {
    pub fn token(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Rem => "%",
            Self::BitAnd => "&",
            Self::BitOr => "|",
            Self::BitXor => "^",
            Self::Shl => "<<",
            Self::Shr => ">>",
            Self::And => "&&",
            Self::Or => "||",
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }

    /// Whether the operator can abort at runtime, by overflow, by a division
    /// by zero, or by an oversized shift. These are the vein the harness
    /// hunts, and the reason integer literals are laundered before they reach
    /// one, so the abort stays a runtime event instead of a compile error.
    pub fn is_fallible(self) -> bool {
        matches!(
            self,
            Self::Add | Self::Sub | Self::Mul | Self::Div | Self::Rem | Self::Shl | Self::Shr
        )
    }

    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            Self::Eq | Self::Ne | Self::Lt | Self::Le | Self::Gt | Self::Ge
        )
    }

    pub fn feature(self) -> &'static str {
        match self {
            Self::Add => "lang-op-add",
            Self::Sub => "lang-op-sub",
            Self::Mul => "lang-op-mul",
            Self::Div => "lang-op-div",
            Self::Rem => "lang-op-rem",
            Self::BitAnd => "lang-op-bitand",
            Self::BitOr => "lang-op-bitor",
            Self::BitXor => "lang-op-bitxor",
            Self::Shl => "lang-op-shl",
            Self::Shr => "lang-op-shr",
            Self::And => "lang-op-and",
            Self::Or => "lang-op-or",
            Self::Eq | Self::Ne | Self::Lt | Self::Le | Self::Gt | Self::Ge => "lang-op-compare",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum UnOp {
    Neg,
    Not,
}

impl UnOp {
    fn token(self) -> &'static str {
        match self {
            Self::Neg => "-",
            Self::Not => "!",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    /// An integer literal. `opaque` routes it through a helper so the compiler
    /// cannot fold it, which is what keeps an overflowing program a runtime
    /// panic instead of a rejected compile.
    IntLit {
        width: IntWidth,
        value: i128,
        opaque: bool,
    },
    /// A float literal. It needs laundering for the same reason an integer
    /// does, even though float arithmetic never aborts: `f32::NEG_INFINITY as
    /// u16` is an integer constant, and the integer lints then fold the `% 0`
    /// or the oversized shift built on top of it into a compile error.
    FloatLit {
        width: FloatWidth,
        token: String,
        opaque: bool,
    },
    BoolLit(bool),
    CharLit(char),
    StrLit(String),
    VecLit {
        elem: Ty,
        items: Vec<Expr>,
    },
    OptLit {
        elem: Ty,
        value: Option<Box<Expr>>,
    },
    /// A map built entry by entry through `insert`, the form real scripts
    /// write. Empty renders as a turbofished `new`.
    MapLit {
        key: Ty,
        value: Ty,
        items: Vec<(Expr, Expr)>,
    },
    SetLit {
        elem: Ty,
        items: Vec<Expr>,
    },
    /// An iterator pipeline, the collect-target shapes among it.
    Pipe(Box<Pipe>),
    /// A call of a generated zero-argument helper function, the vehicle for a
    /// bare `collect` whose target only the helper's return type states.
    FnCall {
        name: String,
        ty: Ty,
    },
    Var {
        name: String,
        ty: Ty,
    },
    Bin {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
        ty: Ty,
    },
    Unary {
        op: UnOp,
        value: Box<Expr>,
        ty: Ty,
    },
    Cast {
        value: Box<Expr>,
        to: Ty,
    },
    /// A call resolved from the catalog. `method` is the catalog key, and the
    /// rendering template comes from the catalog so the table stays the single
    /// source of truth for how a call is written.
    Call {
        method: String,
        recv: Box<Expr>,
        args: Vec<Expr>,
        fish: Option<Ty>,
        ty: Ty,
    },
    If {
        condition: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
        ty: Ty,
    },
}

impl Expr {
    pub fn ty(&self) -> Ty {
        match self {
            Self::IntLit { width, .. } => Ty::Int(*width),
            Self::FloatLit { width, .. } => Ty::Float(*width),
            Self::BoolLit(_) => Ty::Bool,
            Self::CharLit(_) => Ty::Char,
            Self::StrLit(_) => Ty::Str,
            Self::VecLit { elem, .. } => Ty::vec_of(elem.clone()),
            Self::OptLit { elem, .. } => Ty::opt_of(elem.clone()),
            Self::MapLit { key, value, .. } => Ty::map_of(key.clone(), value.clone()),
            Self::SetLit { elem, .. } => Ty::set_of(elem.clone()),
            Self::Pipe(pipe) => pipe.ty(),
            Self::Cast { to, .. } => to.clone(),
            Self::Var { ty, .. }
            | Self::Bin { ty, .. }
            | Self::Unary { ty, .. }
            | Self::Call { ty, .. }
            | Self::If { ty, .. }
            | Self::FnCall { ty, .. } => ty.clone(),
        }
    }

    pub fn render(&self) -> String {
        match self {
            Self::IntLit {
                width,
                value,
                opaque,
            } => render_int_lit(*width, *value, *opaque),
            Self::FloatLit {
                token,
                opaque: false,
                ..
            } => token.clone(),
            Self::FloatLit {
                width,
                token,
                opaque: true,
            } => format!("diff_opaque_{}({token})", width.rust()),
            Self::BoolLit(value) => value.to_string(),
            Self::CharLit(value) => format!("{value:?}"),
            Self::StrLit(value) => format!("String::from({value:?})"),
            Self::VecLit { elem, items } if items.is_empty() => {
                format!("Vec::<{}>::new()", elem.rust())
            }
            Self::VecLit { items, .. } => {
                let rendered: Vec<String> = items.iter().map(Expr::render).collect();
                format!("vec![{}]", rendered.join(", "))
            }
            Self::OptLit { elem, value } => match value {
                Some(inner) => format!("Some({})", inner.render()),
                None => format!("None::<{}>", elem.rust()),
            },
            Self::MapLit { key, value, items } if items.is_empty() => {
                format!("HashMap::<{}, {}>::new()", key.rust(), value.rust())
            }
            Self::MapLit { key, value, items } => {
                let inserts: Vec<String> = items
                    .iter()
                    .map(|(entry_key, entry_value)| {
                        format!(
                            "diff_map.insert({}, {});",
                            entry_key.render(),
                            entry_value.render()
                        )
                    })
                    .collect();
                format!(
                    "({{ let mut diff_map: HashMap<{}, {}> = HashMap::new(); {} diff_map }})",
                    key.rust(),
                    value.rust(),
                    inserts.join(" ")
                )
            }
            Self::SetLit { elem, items } if items.is_empty() => {
                format!("HashSet::<{}>::new()", elem.rust())
            }
            Self::SetLit { elem, items } => {
                let inserts: Vec<String> = items
                    .iter()
                    .map(|item| format!("diff_set.insert({});", item.render()))
                    .collect();
                format!(
                    "({{ let mut diff_set: HashSet<{}> = HashSet::new(); {} diff_set }})",
                    elem.rust(),
                    inserts.join(" ")
                )
            }
            Self::Pipe(pipe) => pipe.render(),
            Self::FnCall { name, .. } => format!("{name}()"),
            // A non-copy binding is always read through a clone, which is what
            // keeps generated programs free of move and borrow errors without
            // the generator having to track liveness.
            Self::Var { name, ty } if !ty.is_copy() => format!("{name}.clone()"),
            Self::Var { name, .. } => name.clone(),
            Self::Bin {
                op, left, right, ..
            } => {
                format!("({} {} {})", left.render(), op.token(), right.render())
            }
            Self::Unary { op, value, .. } => format!("({}{})", op.token(), value.render()),
            Self::Cast { value, to } => format!("({} as {})", value.render(), to.rust()),
            Self::Call {
                method,
                recv,
                args,
                fish,
                ..
            } => render_call(method, recv, args, fish.as_ref()),
            // Always parenthesized. Bare `if a { x } else { y }.len()` parses
            // as `if a { x } else { y.len() }`, so the source would stop
            // matching the tree and a shrink of the receiver would rewrite a
            // different expression than the one it aimed at.
            Self::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => format!(
                "(if {} {{ {} }} else {{ {} }})",
                condition.render(),
                then_expr.render(),
                else_expr.render()
            ),
        }
    }

    pub fn uses_any(&self, names: &BTreeSet<String>) -> bool {
        match self {
            Self::Var { name, .. } => names.contains(name),
            Self::IntLit { .. }
            | Self::FloatLit { .. }
            | Self::BoolLit(_)
            | Self::CharLit(_)
            | Self::StrLit(_)
            | Self::FnCall { .. } => false,
            Self::VecLit { items, .. } | Self::SetLit { items, .. } => {
                items.iter().any(|item| item.uses_any(names))
            }
            Self::OptLit { value, .. } => value.as_ref().is_some_and(|inner| inner.uses_any(names)),
            Self::MapLit { items, .. } => items
                .iter()
                .any(|(key, value)| key.uses_any(names) || value.uses_any(names)),
            Self::Pipe(pipe) => pipe.uses_any(names),
            Self::Bin { left, right, .. } => left.uses_any(names) || right.uses_any(names),
            Self::Unary { value, .. } | Self::Cast { value, .. } => value.uses_any(names),
            Self::Call { recv, args, .. } => {
                recv.uses_any(names) || args.iter().any(|arg| arg.uses_any(names))
            }
            Self::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => {
                condition.uses_any(names) || then_expr.uses_any(names) || else_expr.uses_any(names)
            }
        }
    }

    /// Whether the subtree contains an operator that can abort at runtime.
    /// Drives the decision to launder integer literals.
    pub fn has_fallible_op(&self) -> bool {
        match self {
            Self::Bin {
                op, left, right, ..
            } => op.is_fallible() || left.has_fallible_op() || right.has_fallible_op(),
            Self::Unary { op, value, .. } => matches!(op, UnOp::Neg) || value.has_fallible_op(),
            Self::Cast { value, .. } => value.has_fallible_op(),
            Self::VecLit { items, .. } | Self::SetLit { items, .. } => {
                items.iter().any(Expr::has_fallible_op)
            }
            Self::OptLit { value, .. } => {
                value.as_ref().is_some_and(|inner| inner.has_fallible_op())
            }
            Self::MapLit { items, .. } => items
                .iter()
                .any(|(key, value)| key.has_fallible_op() || value.has_fallible_op()),
            // Every catalog call reaches std code the const propagator can look
            // through, `pow` and `sum` among them, so a call counts as fallible.
            // A pipe carries `sum` and `fold`, a helper body is out of sight,
            // both count the same way.
            Self::Call { .. } | Self::Pipe(_) | Self::FnCall { .. } => true,
            Self::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => {
                condition.has_fallible_op()
                    || then_expr.has_fallible_op()
                    || else_expr.has_fallible_op()
            }
            _ => false,
        }
    }

    /// Mark every integer literal in the subtree as needing the opaque helper.
    pub fn make_opaque(&mut self) {
        match self {
            Self::IntLit { opaque, .. } | Self::FloatLit { opaque, .. } => *opaque = true,
            Self::VecLit { items, .. } | Self::SetLit { items, .. } => {
                for item in items {
                    item.make_opaque();
                }
            }
            Self::OptLit {
                value: Some(inner), ..
            } => inner.make_opaque(),
            Self::MapLit { items, .. } => {
                for (key, value) in items {
                    key.make_opaque();
                    value.make_opaque();
                }
            }
            Self::Pipe(pipe) => pipe.make_opaque(),
            Self::Bin { left, right, .. } => {
                left.make_opaque();
                right.make_opaque();
            }
            Self::Unary { value, .. } | Self::Cast { value, .. } => value.make_opaque(),
            Self::Call { recv, args, .. } => {
                recv.make_opaque();
                for arg in args {
                    arg.make_opaque();
                }
            }
            Self::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => {
                condition.make_opaque();
                then_expr.make_opaque();
                else_expr.make_opaque();
            }
            _ => {}
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
            Self::VecLit { items, .. } | Self::SetLit { items, .. } => {
                for item in items {
                    item.helpers(out);
                }
            }
            Self::OptLit {
                value: Some(inner), ..
            } => inner.helpers(out),
            Self::MapLit { items, .. } => {
                for (key, value) in items {
                    key.helpers(out);
                    value.helpers(out);
                }
            }
            Self::Pipe(pipe) => pipe.helpers(out),
            Self::Bin { left, right, .. } => {
                left.helpers(out);
                right.helpers(out);
            }
            Self::Unary { value, .. } | Self::Cast { value, .. } => value.helpers(out),
            Self::Call { recv, args, .. } => {
                recv.helpers(out);
                for arg in args {
                    arg.helpers(out);
                }
            }
            Self::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => {
                condition.helpers(out);
                then_expr.helpers(out);
                else_expr.helpers(out);
            }
            _ => {}
        }
    }

    pub fn features(&self, out: &mut BTreeSet<&'static str>) {
        out.insert(self.ty().feature());
        match self {
            Self::Bin {
                op, left, right, ..
            } => {
                out.insert(op.feature());
                left.features(out);
                right.features(out);
            }
            Self::Unary { value, .. } => {
                out.insert("lang-unary");
                value.features(out);
            }
            Self::Cast { value, .. } => {
                out.insert("lang-cast");
                value.features(out);
            }
            Self::Call {
                method, recv, args, ..
            } => {
                out.insert("lang-call");
                if let Some(entry) = lookup(method) {
                    out.insert(entry.name);
                }
                recv.features(out);
                for arg in args {
                    arg.features(out);
                }
            }
            Self::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => {
                out.insert("lang-if");
                condition.features(out);
                then_expr.features(out);
                else_expr.features(out);
            }
            Self::VecLit { items, .. } | Self::SetLit { items, .. } => {
                for item in items {
                    item.features(out);
                }
            }
            Self::OptLit {
                value: Some(inner), ..
            } => inner.features(out),
            Self::MapLit { items, .. } => {
                for (key, value) in items {
                    key.features(out);
                    value.features(out);
                }
            }
            Self::Pipe(pipe) => pipe.features(out),
            Self::FnCall { .. } => {
                out.insert("lang-fn-call");
            }
            _ => {}
        }
    }

    /// Whether the tree contains a call of the helper function `name`.
    pub fn calls_fn(&self, name: &str) -> bool {
        if let Self::FnCall { name: called, .. } = self
            && called == name
        {
            return true;
        }
        self.children().iter().any(|child| child.calls_fn(name))
    }

    /// Direct children, so shrinking can hoist a same-typed subtree.
    fn children(&self) -> Vec<&Expr> {
        match self {
            Self::Bin { left, right, .. } => vec![left, right],
            Self::Unary { value, .. } | Self::Cast { value, .. } => vec![value],
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
            Self::VecLit { items, .. } | Self::SetLit { items, .. } => items.iter().collect(),
            Self::OptLit { value, .. } => value.iter().map(|inner| &**inner).collect(),
            Self::MapLit { items, .. } => {
                items.iter().flat_map(|(key, value)| [key, value]).collect()
            }
            Self::Pipe(pipe) => pipe.exprs(),
            _ => Vec::new(),
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
        if let Self::VecLit { items, .. } = self
            && !items.is_empty()
        {
            let mut shorter = self.clone();
            if let Self::VecLit { items, .. } = &mut shorter {
                items.pop();
            }
            candidates.push(shorter);
        }
        if let Self::MapLit { items, .. } = self
            && !items.is_empty()
        {
            let mut shorter = self.clone();
            if let Self::MapLit { items, .. } = &mut shorter {
                items.pop();
            }
            candidates.push(shorter);
        }
        if let Self::SetLit { items, .. } = self
            && !items.is_empty()
        {
            let mut shorter = self.clone();
            if let Self::SetLit { items, .. } = &mut shorter {
                items.pop();
            }
            candidates.push(shorter);
        }
        if let Self::Pipe(pipe) = self {
            candidates.extend(pipe.shrinks().into_iter().map(|p| Self::Pipe(Box::new(p))));
        }
        candidates
    }
}

/// The opaque helper functions a program needs, emitted only when used.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Helper {
    I64,
    U64,
    F32,
    F64,
}

impl Helper {
    pub fn definition(self) -> &'static str {
        match self {
            Self::I64 => "fn diff_opaque_i64(value: i64) -> i64 {\n    value\n}\n\n",
            Self::U64 => "fn diff_opaque_u64(value: u64) -> u64 {\n    value\n}\n\n",
            Self::F32 => "fn diff_opaque_f32(value: f32) -> f32 {\n    value\n}\n\n",
            Self::F64 => "fn diff_opaque_f64(value: f64) -> f64 {\n    value\n}\n\n",
        }
    }
}

fn render_int_lit(width: IntWidth, value: i128, opaque: bool) -> String {
    if !opaque {
        return if value < 0 {
            format!("({value}{})", width.rust())
        } else {
            format!("{value}{}", width.rust())
        };
    }
    if width.is_signed() {
        format!("(diff_opaque_i64({value}) as {})", width.rust())
    } else {
        format!("(diff_opaque_u64({value}) as {})", width.rust())
    }
}

pub fn lookup(name: &str) -> Option<&'static Method> {
    METHODS.iter().find(|method| method.name == name)
}

fn render_call(method: &str, recv: &Expr, args: &[Expr], fish: Option<&Ty>) -> String {
    let Some(entry) = lookup(method) else {
        return recv.render();
    };
    let rendered_args: Vec<String> = args.iter().map(Expr::render).collect();
    let recv_ty = recv.ty();
    let elem = recv_ty.elem().map(Ty::rust).unwrap_or_default();
    let (key, val) = match recv_ty.key_val() {
        Some((key, value)) => (key.rust(), value.rust()),
        None => (String::new(), String::new()),
    };
    let fish = fish.map(Ty::rust).unwrap_or_default();
    fill(
        entry.template,
        &recv.render(),
        &rendered_args,
        &elem,
        &fish,
        &key,
        &val,
    )
}

/// Expand a catalog template. `{{` and `}}` escape a literal brace, the same
/// way `format!` does, so a template can carry a block expression.
fn fill(
    template: &str,
    recv: &str,
    args: &[String],
    elem: &str,
    fish: &str,
    key_ty: &str,
    val: &str,
) -> String {
    let mut out = String::with_capacity(template.len() + recv.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '}' {
            if chars.peek() == Some(&'}') {
                chars.next();
            }
            out.push('}');
            continue;
        }
        if c != '{' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'{') {
            chars.next();
            out.push('{');
            continue;
        }
        let mut key = String::new();
        for c in chars.by_ref() {
            if c == '}' {
                break;
            }
            key.push(c);
        }
        match key.as_str() {
            "r" => out.push_str(recv),
            "E" => out.push_str(elem),
            "T" => out.push_str(fish),
            "K" => out.push_str(key_ty),
            "V" => out.push_str(val),
            index => match index.parse::<usize>() {
                Ok(index) if index < args.len() => out.push_str(&args[index]),
                _ => {}
            },
        }
    }
    out
}

/// The simplest expression of a type, the target every shrink step aims at.
pub fn minimal(ty: &Ty) -> Expr {
    match ty {
        Ty::Int(width) => Expr::IntLit {
            width: *width,
            value: 0,
            opaque: false,
        },
        Ty::Float(width) => Expr::FloatLit {
            width: *width,
            token: match width {
                FloatWidth::F32 => "0.0f32".to_string(),
                FloatWidth::F64 => "0.0f64".to_string(),
            },
            opaque: false,
        },
        Ty::Bool => Expr::BoolLit(false),
        Ty::Char => Expr::CharLit('a'),
        Ty::Str => Expr::StrLit(String::new()),
        Ty::Vec(elem) => Expr::VecLit {
            elem: (**elem).clone(),
            items: Vec::new(),
        },
        Ty::Opt(elem) => Expr::OptLit {
            elem: (**elem).clone(),
            value: None,
        },
        Ty::Map(key, value) => Expr::MapLit {
            key: (**key).clone(),
            value: (**value).clone(),
            items: Vec::new(),
        },
        Ty::Set(elem) => Expr::SetLit {
            elem: (**elem).clone(),
            items: Vec::new(),
        },
    }
}
