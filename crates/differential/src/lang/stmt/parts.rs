//! The small parts a statement is built from, annotations, print forms and closure shapes.

use serde::{Deserialize, Serialize};

use crate::lang::expr::Expr;
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
