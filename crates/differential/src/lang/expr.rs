//! Every node carries its result type, so the generator, renderer and shrinker agree without
//! re-running inference.

use serde::{Deserialize, Serialize};

use crate::lang::catalog::{METHODS, Method};
use crate::lang::pat::Pat;
use crate::lang::pipe::Pipe;
use crate::lang::stmt::Stmt;
use crate::lang::ty::{FloatWidth, IntWidth, StdErr, Ty};
use crate::lang::user::{MethodKind, UserShape};

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

    /// Whether the operator can abort at runtime. Literals are laundered before they reach one,
    /// so the abort stays a runtime event.
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

    pub fn has_compound(self) -> bool {
        !self.is_comparison() && !matches!(self, Self::And | Self::Or)
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
    pub(super) fn token(self) -> &'static str {
        match self {
            Self::Neg => "-",
            Self::Not => "!",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Arm {
    pub pat: Pat,
    pub guard: Option<Expr>,
    pub body: Expr,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    /// `opaque` routes it through a helper so the compiler can't fold it
    IntLit {
        width: IntWidth,
        value: i128,
        opaque: bool,
    },
    /// Unsuffixed, so `i32` by default. `opaque` wraps it in the `if diff_opaque_true()` shield,
    /// which hides it from the overflow lint without naming a type.
    BareInt {
        value: i128,
        opaque: bool,
    },
    /// laundered too, `f32::NEG_INFINITY as u16` is an integer constant the lints fold
    FloatLit {
        width: FloatWidth,
        token: String,
        opaque: bool,
    },
    /// unsuffixed, `f64` by default
    BareFloat {
        token: String,
        opaque: bool,
    },
    /// `opaque` keeps `rustc` from folding a shift by `'7' as u32`
    BoolLit {
        value: bool,
        opaque: bool,
    },
    CharLit {
        value: char,
        opaque: bool,
    },
    StrLit(String),
    VecLit {
        elem: Ty,
        items: Vec<Expr>,
    },
    /// `vec![item; count]`, every element must own its own storage
    VecRepeat {
        elem: Ty,
        item: Box<Expr>,
        count: u8,
    },
    OptLit {
        elem: Ty,
        value: Option<Box<Expr>>,
    },
    /// built through `insert`, empty renders as a turbofished `new`
    MapLit {
        key: Ty,
        value: Ty,
        items: Vec<(Expr, Expr)>,
    },
    SetLit {
        elem: Ty,
        items: Vec<Expr>,
    },
    TupleLit(Vec<Expr>),
    /// `Ok::<T, E>(..)` or `Err::<T, E>(..)`, both types pinned
    ResLit {
        ok: Ty,
        err: Ty,
        value: Result<Box<Expr>, Box<Expr>>,
    },
    /// made by a parse that fails
    StdErrLit(StdErr),
    /// `Name { a: .., b: .. }`, or with `update` the first `fields.len()` fields written and
    /// `..Default::default()` for the rest
    StructLit {
        shape: Box<UserShape>,
        fields: Vec<Expr>,
        update: bool,
    },
    EnumLit {
        shape: Box<UserShape>,
        variant: usize,
        payload: Vec<Expr>,
    },
    /// `<T>::default()`
    DefaultOf(Ty),
    /// an iterator pipeline
    Pipe(Box<Pipe>),
    /// `by_ref` marks the arguments the function takes by `&`
    FnCall {
        name: String,
        args: Vec<Expr>,
        #[serde(default)]
        by_ref: Vec<bool>,
        ty: Ty,
    },
    ClosureCall {
        name: String,
        args: Vec<Expr>,
        ty: Ty,
    },
    Var {
        name: String,
        ty: Ty,
    },
    /// `opaque` shields it from the overflow lint like a bare literal
    ConstRef {
        name: String,
        ty: Ty,
        opaque: bool,
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
    /// `method` is the catalog key, the template comes from the catalog
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
    Field {
        base: Box<Expr>,
        index: usize,
        ty: Ty,
    },
    TupleField {
        base: Box<Expr>,
        index: usize,
        ty: Ty,
    },
    /// `v[i]`, panics out of bounds
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
        ty: Ty,
    },
    /// `base.name(args)`, or `Type::name(args)` with `Assoc` kind
    Method {
        owner: Box<UserShape>,
        name: String,
        kind: MethodKind,
        base: Option<Box<Expr>>,
        args: Vec<Expr>,
        ty: Ty,
    },
    /// `base.diff_describe()` through the program local trait
    TraitCall {
        base: Box<Expr>,
    },
    /// `helper(&mut closure, arg)`, a closure handed to a generic helper
    ApplyCall {
        helper: String,
        closure: String,
        arg: Box<Expr>,
        ty: Ty,
    },
    /// `value?`
    Try {
        value: Box<Expr>,
        ty: Ty,
    },
    /// `To::from(value)`, or `value.into()` when `bare`
    Into {
        value: Box<Expr>,
        to: Ty,
        bare: bool,
    },
    Match {
        scrutinee: Box<Expr>,
        /// a slice view, so bindings arrive as references and each arm clones them
        by_ref: bool,
        arms: Vec<Arm>,
        ty: Ty,
    },
    /// `{ stmts; tail }`
    Block {
        stmts: Vec<Stmt>,
        tail: Box<Expr>,
    },
}

/// Emitted only when used.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum Helper {
    I64,
    U64,
    F32,
    F64,
    True,
    Char,
}

impl Helper {
    pub fn definition(self) -> &'static str {
        match self {
            Self::I64 => "fn diff_opaque_i64(value: i64) -> i64 {\n    value\n}\n\n",
            Self::U64 => "fn diff_opaque_u64(value: u64) -> u64 {\n    value\n}\n\n",
            Self::F32 => "fn diff_opaque_f32(value: f32) -> f32 {\n    value\n}\n\n",
            Self::F64 => "fn diff_opaque_f64(value: f64) -> f64 {\n    value\n}\n\n",
            Self::True => "fn diff_opaque_true() -> bool {\n    true\n}\n\n",
            Self::Char => "fn diff_opaque_char(value: char) -> char {\n    value\n}\n\n",
        }
    }
}

pub fn lookup(name: &str) -> Option<&'static Method> {
    METHODS.iter().find(|method| method.name == name)
}

/// Un-bare every literal in the tree. A receiver like `(if c { 0 } else { 0 })` is as ambiguous
/// as a bare `0`.
pub fn unbare_deep(mut expr: Expr) -> Expr {
    expr = unbare(expr);
    for child in expr.children_mut() {
        let taken = std::mem::replace(
            child,
            Expr::BoolLit {
                value: false,
                opaque: false,
            },
        );
        *child = unbare_deep(taken);
    }
    expr
}

pub fn unbare(expr: Expr) -> Expr {
    match expr {
        Expr::BareInt { value, opaque } => Expr::IntLit {
            width: IntWidth::I32,
            value,
            opaque,
        },
        Expr::BareFloat { token, opaque } => {
            let token = match token.strip_suffix(')') {
                Some(inner) => format!("{inner}f64)"),
                None => format!("{token}f64"),
            };
            Expr::FloatLit {
                width: FloatWidth::F64,
                token,
                opaque,
            }
        }
        other => other,
    }
}

/// The target every shrink step aims at.
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
        Ty::Bool => Expr::BoolLit {
            value: false,
            opaque: false,
        },
        Ty::Char => Expr::CharLit {
            value: 'a',
            opaque: false,
        },
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
        Ty::Tuple(items) => Expr::TupleLit(items.iter().map(minimal).collect()),
        Ty::Res(ok, err) => Expr::ResLit {
            ok: (**ok).clone(),
            err: (**err).clone(),
            value: Ok(Box::new(minimal(ok))),
        },
        Ty::StdErr(err) => Expr::StdErrLit(*err),
        Ty::User(shape) => {
            if shape.is_enum() {
                let variant = &shape.variants()[0];
                Expr::EnumLit {
                    shape: shape.clone(),
                    variant: 0,
                    payload: variant.payload.iter().map(minimal).collect(),
                }
            } else {
                Expr::StructLit {
                    shape: shape.clone(),
                    fields: shape.fields().iter().map(|f| minimal(&f.ty)).collect(),
                    update: false,
                }
            }
        }
    }
}
