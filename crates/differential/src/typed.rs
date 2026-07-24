use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum GeneratedType {
    I64,
    F64,
    Bool,
    String,
    VecI64,
    OptionI64,
}

impl GeneratedType {
    pub fn rust(self) -> &'static str {
        match self {
            Self::I64 => "i64",
            Self::F64 => "f64",
            Self::Bool => "bool",
            Self::String => "String",
            Self::VecI64 => "Vec<i64>",
            Self::OptionI64 => "Option<i64>",
        }
    }

    pub(crate) fn is_owned(self) -> bool {
        matches!(self, Self::String | Self::VecI64)
    }
}

/// Plain `+ - * / %` applied in a narrow integer type between two casts from
/// i64. Overflow panics under the compiler's debug semantics and division by
/// zero panics in any width, both places an interpreter that computes in i64
/// can silently diverge.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum NarrowOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

impl NarrowOp {
    pub(crate) fn token(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Rem => "%",
        }
    }
}

/// Integer types a value can be narrowed to with `as`. The interpreter keeps
/// every integer as an i64, so a narrowing cast that truncates in compiled Rust
/// is a divergence the harness hunts for.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum IntCast {
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    USize,
}

impl IntCast {
    pub(crate) fn rust(self) -> &'static str {
        match self {
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::USize => "usize",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum GeneratedExpr {
    I64(i64),
    Bool(bool),
    Text(String),
    Variable {
        name: String,
        ty: GeneratedType,
    },
    Add(Box<Self>, Box<Self>),
    Subtract(Box<Self>, Box<Self>),
    Multiply(Box<Self>, Box<Self>),
    Equal(Box<Self>, Box<Self>),
    Less(Box<Self>, Box<Self>),
    And(Box<Self>, Box<Self>),
    Or(Box<Self>, Box<Self>),
    Not(Box<Self>),
    If {
        condition: Box<Self>,
        then_expr: Box<Self>,
        else_expr: Box<Self>,
        ty: GeneratedType,
    },
    Concat(Box<Self>, Box<Self>),
    Uppercase(Box<Self>),
    Replace {
        value: Box<Self>,
        from: String,
        to: String,
    },
    FormatI64(Box<Self>),
    DebugVec(Box<Self>),
    VecLiteral(Vec<Self>),
    VecMap {
        values: Box<Self>,
        binding: String,
        body: Box<Self>,
    },
    VecFilter {
        values: Box<Self>,
        binding: String,
        predicate: Box<Self>,
    },
    VecReverse(Box<Self>),
    VecAppend {
        values: Box<Self>,
        value: Box<Self>,
    },
    VecLen(Box<Self>),
    VecGetOr {
        values: Box<Self>,
        index: usize,
        default: Box<Self>,
    },
    Some(Box<Self>),
    None,
    OptionMap {
        option: Box<Self>,
        binding: String,
        body: Box<Self>,
    },
    OptionFilter {
        option: Box<Self>,
        binding: String,
        predicate: Box<Self>,
    },
    OptionUnwrapOr {
        option: Box<Self>,
        default: Box<Self>,
    },
    OptionIsSome(Box<Self>),
    MatchOption {
        option: Box<Self>,
        binding: String,
        some: Box<Self>,
        none: Box<Self>,
        ty: GeneratedType,
    },
    ClosureCall {
        binding: String,
        input: Box<Self>,
        body: Box<Self>,
        ty: GeneratedType,
    },
    /// `(value as u8) as i64` and friends. Truncates in compiled Rust, keeps
    /// the full i64 in the interpreter.
    Cast(Box<Self>, IntCast),
    /// Plain `+ - *`, which panic on overflow under the compiler's overflow
    /// checks and wrap in the interpreter. The left operand is always a
    /// variable so the operands are never both constant, which keeps the
    /// compiler's const-overflow lint from rejecting the program.
    RawAdd(Box<Self>, Box<Self>),
    RawSub(Box<Self>, Box<Self>),
    RawMul(Box<Self>, Box<Self>),
    /// Plain `/ %`, which panic on a zero divisor or on `i64::MIN / -1`.
    RawDiv(Box<Self>, Box<Self>),
    RawRem(Box<Self>, Box<Self>),
    /// `values[index]`, which panics out of bounds.
    Index {
        values: Box<Self>,
        index: usize,
    },
    /// `option.unwrap()`, which panics on `None`.
    Unwrap(Box<Self>),
    /// An f64 literal kept as its exact source token, so serialization and
    /// equality stay simple and the rendered program reproduces the value bit
    /// for bit. Tokens containing `::` are constants such as `f64::NAN`.
    F64(String),
    FAdd(Box<Self>, Box<Self>),
    FSub(Box<Self>, Box<Self>),
    FMul(Box<Self>, Box<Self>),
    /// Float division never panics; a zero divisor gives infinity or NaN.
    FDiv(Box<Self>, Box<Self>),
    FLess(Box<Self>, Box<Self>),
    FEq(Box<Self>, Box<Self>),
    /// `(value as f64)`.
    I64ToF64(Box<Self>),
    /// `(value as i64)`, which saturates and maps NaN to zero in real Rust.
    F64ToI64(Box<Self>),
    /// `format!("{}", value)` over an f64, the Display path.
    FormatF64(Box<Self>),
    /// `format!("{:?}", value)` over an f64, the Debug path, where whole
    /// numbers must keep their `.0`.
    DebugF64(Box<Self>),
    /// See [`NarrowOp`].
    NarrowArith {
        target: IntCast,
        op: NarrowOp,
        left: Box<Self>,
        right: Box<Self>,
    },
    /// `format!("{SPEC}", value)` with a non-trivial format spec, width, fill,
    /// alignment, sign, zero padding, precision, radix, or exponent.
    FormatSpec {
        spec: String,
        value: Box<Self>,
    },
}

impl GeneratedExpr {
    pub fn variable(name: impl Into<String>, ty: GeneratedType) -> Self {
        Self::Variable {
            name: name.into(),
            ty,
        }
    }

    pub fn ty(&self) -> GeneratedType {
        match self {
            Self::I64(_)
            | Self::Add(..)
            | Self::Subtract(..)
            | Self::Multiply(..)
            | Self::VecLen(_)
            | Self::VecGetOr { .. }
            | Self::OptionUnwrapOr { .. }
            | Self::Cast(..)
            | Self::RawAdd(..)
            | Self::RawSub(..)
            | Self::RawMul(..)
            | Self::RawDiv(..)
            | Self::RawRem(..)
            | Self::Index { .. }
            | Self::Unwrap(_)
            | Self::F64ToI64(_)
            | Self::NarrowArith { .. } => GeneratedType::I64,
            Self::F64(_)
            | Self::FAdd(..)
            | Self::FSub(..)
            | Self::FMul(..)
            | Self::FDiv(..)
            | Self::I64ToF64(_) => GeneratedType::F64,
            Self::Bool(_)
            | Self::Equal(..)
            | Self::Less(..)
            | Self::And(..)
            | Self::Or(..)
            | Self::Not(_)
            | Self::FLess(..)
            | Self::FEq(..)
            | Self::OptionIsSome(_) => GeneratedType::Bool,
            Self::Text(_)
            | Self::Concat(..)
            | Self::Uppercase(_)
            | Self::Replace { .. }
            | Self::FormatI64(_)
            | Self::FormatF64(_)
            | Self::DebugF64(_)
            | Self::FormatSpec { .. }
            | Self::DebugVec(_) => GeneratedType::String,
            Self::VecLiteral(_)
            | Self::VecMap { .. }
            | Self::VecFilter { .. }
            | Self::VecReverse(_)
            | Self::VecAppend { .. } => GeneratedType::VecI64,
            Self::Some(_) | Self::None | Self::OptionMap { .. } | Self::OptionFilter { .. } => {
                GeneratedType::OptionI64
            }
            Self::Variable { ty, .. }
            | Self::If { ty, .. }
            | Self::MatchOption { ty, .. }
            | Self::ClosureCall { ty, .. } => *ty,
        }
    }

    pub fn uses(&self, name: &str) -> bool {
        match self {
            Self::Variable { name: variable, .. } => variable == name,
            Self::Add(left, right)
            | Self::Subtract(left, right)
            | Self::Multiply(left, right)
            | Self::Equal(left, right)
            | Self::Less(left, right)
            | Self::And(left, right)
            | Self::Or(left, right)
            | Self::Concat(left, right) => left.uses(name) || right.uses(name),
            Self::Not(value)
            | Self::Uppercase(value)
            | Self::FormatI64(value)
            | Self::DebugVec(value)
            | Self::VecReverse(value)
            | Self::VecLen(value)
            | Self::Some(value)
            | Self::OptionIsSome(value) => value.uses(name),
            Self::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => condition.uses(name) || then_expr.uses(name) || else_expr.uses(name),
            Self::Replace { value, .. } => value.uses(name),
            Self::VecLiteral(values) => values.iter().any(|value| value.uses(name)),
            Self::VecMap { values, body, .. }
            | Self::VecFilter {
                values,
                predicate: body,
                ..
            }
            | Self::OptionMap {
                option: values,
                body,
                ..
            }
            | Self::OptionFilter {
                option: values,
                predicate: body,
                ..
            } => values.uses(name) || body.uses(name),
            Self::VecAppend { values, value } => values.uses(name) || value.uses(name),
            Self::VecGetOr {
                values, default, ..
            }
            | Self::OptionUnwrapOr {
                option: values,
                default,
            } => values.uses(name) || default.uses(name),
            Self::MatchOption {
                option, some, none, ..
            } => option.uses(name) || some.uses(name) || none.uses(name),
            Self::ClosureCall { input, body, .. } => input.uses(name) || body.uses(name),
            Self::RawAdd(left, right)
            | Self::RawSub(left, right)
            | Self::RawMul(left, right)
            | Self::RawDiv(left, right)
            | Self::RawRem(left, right)
            | Self::FAdd(left, right)
            | Self::FSub(left, right)
            | Self::FMul(left, right)
            | Self::FDiv(left, right)
            | Self::FLess(left, right)
            | Self::FEq(left, right)
            | Self::NarrowArith { left, right, .. } => left.uses(name) || right.uses(name),
            Self::Cast(value, _)
            | Self::Unwrap(value)
            | Self::I64ToF64(value)
            | Self::F64ToI64(value)
            | Self::FormatF64(value)
            | Self::DebugF64(value)
            | Self::FormatSpec { value, .. } => value.uses(name),
            Self::Index { values, .. } => values.uses(name),
            Self::I64(_) | Self::Bool(_) | Self::Text(_) | Self::F64(_) | Self::None => false,
        }
    }

    pub fn shrinks(&self) -> Vec<Self> {
        crate::typed_shrink::shrink(self)
    }

    pub fn shape(&self, output: &mut String) {
        output.push_str(self.shape_name());
        output.push('(');
        for child in self.children() {
            child.shape(output);
            output.push(',');
        }
        output.push(')');
    }

    pub fn features(&self, output: &mut BTreeSet<&'static str>) {
        output.insert(self.shape_name());
        for child in self.children() {
            child.features(output);
        }
    }

    fn shape_name(&self) -> &'static str {
        match self {
            Self::I64(_) => "i64",
            Self::Bool(_) => "bool",
            Self::Text(_) => "text",
            Self::Variable { ty, .. } => match ty {
                GeneratedType::I64 => "var-i64",
                GeneratedType::F64 => "var-f64",
                GeneratedType::Bool => "var-bool",
                GeneratedType::String => "var-string",
                GeneratedType::VecI64 => "var-vec",
                GeneratedType::OptionI64 => "var-option",
            },
            Self::Add(..) => "add",
            Self::Subtract(..) => "subtract",
            Self::Multiply(..) => "multiply",
            Self::Equal(..) => "equal",
            Self::Less(..) => "less",
            Self::And(..) => "and",
            Self::Or(..) => "or",
            Self::Not(_) => "not",
            Self::If { .. } => "if",
            Self::Concat(..) => "concat",
            Self::Uppercase(_) => "uppercase",
            Self::Replace { .. } => "replace",
            Self::FormatI64(_) => "format-i64",
            Self::DebugVec(_) => "debug-vec",
            Self::VecLiteral(_) => "vec-literal",
            Self::VecMap { .. } => "vec-map",
            Self::VecFilter { .. } => "vec-filter",
            Self::VecReverse(_) => "vec-reverse",
            Self::VecAppend { .. } => "vec-append",
            Self::VecLen(_) => "vec-len",
            Self::VecGetOr { .. } => "vec-get",
            Self::Some(_) => "some",
            Self::None => "none",
            Self::OptionMap { .. } => "option-map",
            Self::OptionFilter { .. } => "option-filter",
            Self::OptionUnwrapOr { .. } => "option-unwrap",
            Self::OptionIsSome(_) => "option-is-some",
            Self::MatchOption { .. } => "match-option",
            Self::ClosureCall { .. } => "closure-call",
            Self::Cast(..) => "cast",
            Self::RawAdd(..) => "raw-add",
            Self::RawSub(..) => "raw-sub",
            Self::RawMul(..) => "raw-mul",
            Self::RawDiv(..) => "raw-div",
            Self::RawRem(..) => "raw-rem",
            Self::Index { .. } => "index",
            Self::Unwrap(_) => "unwrap",
            Self::F64(_) => "f64",
            Self::FAdd(..) => "f64-add",
            Self::FSub(..) => "f64-sub",
            Self::FMul(..) => "f64-mul",
            Self::FDiv(..) => "f64-div",
            Self::FLess(..) => "f64-less",
            Self::FEq(..) => "f64-eq",
            Self::I64ToF64(_) => "cast-i64-f64",
            Self::F64ToI64(_) => "cast-f64-i64",
            Self::FormatF64(_) => "format-f64",
            Self::DebugF64(_) => "debug-f64",
            Self::NarrowArith { op, .. } => match op {
                NarrowOp::Add => "narrow-add",
                NarrowOp::Sub => "narrow-sub",
                NarrowOp::Mul => "narrow-mul",
                NarrowOp::Div => "narrow-div",
                NarrowOp::Rem => "narrow-rem",
            },
            Self::FormatSpec { .. } => "format-spec",
        }
    }

    /// Every node of the tree in pre-order, itself included. The donor side
    /// of a splice picks subtrees from this list.
    pub fn nodes(&self) -> Vec<&Self> {
        let mut nodes = vec![self];
        let mut index = 0;
        while index < nodes.len() {
            let children = nodes[index].children();
            nodes.extend(children);
            index += 1;
        }
        nodes
    }

    /// The `n`th node in the same pre-order `nodes` uses, mutably, so a
    /// splice can replace the subtree it picked by index.
    pub fn nth_node_mut(&mut self, n: usize) -> Option<&mut Self> {
        let mut remaining = n;
        let mut stack = vec![self];
        while let Some(node) = stack.pop() {
            if remaining == 0 {
                return Some(node);
            }
            remaining -= 1;
            let mut children = node.children_mut();
            children.reverse();
            stack.extend(children);
        }
        None
    }

    pub fn children_mut(&mut self) -> Vec<&mut Self> {
        match self {
            Self::Add(left, right)
            | Self::Subtract(left, right)
            | Self::Multiply(left, right)
            | Self::Equal(left, right)
            | Self::Less(left, right)
            | Self::And(left, right)
            | Self::Or(left, right)
            | Self::Concat(left, right)
            | Self::RawAdd(left, right)
            | Self::RawSub(left, right)
            | Self::RawMul(left, right)
            | Self::RawDiv(left, right)
            | Self::RawRem(left, right)
            | Self::FAdd(left, right)
            | Self::FSub(left, right)
            | Self::FMul(left, right)
            | Self::FDiv(left, right)
            | Self::FLess(left, right)
            | Self::FEq(left, right)
            | Self::NarrowArith { left, right, .. } => vec![left, right],
            Self::Not(value)
            | Self::Uppercase(value)
            | Self::FormatI64(value)
            | Self::DebugVec(value)
            | Self::VecReverse(value)
            | Self::VecLen(value)
            | Self::Some(value)
            | Self::OptionIsSome(value)
            | Self::Cast(value, _)
            | Self::Unwrap(value)
            | Self::I64ToF64(value)
            | Self::F64ToI64(value)
            | Self::FormatF64(value)
            | Self::DebugF64(value)
            | Self::FormatSpec { value, .. }
            | Self::Replace { value, .. } => vec![value],
            Self::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => vec![condition, then_expr, else_expr],
            Self::VecLiteral(values) => values.iter_mut().collect(),
            Self::VecMap { values, body, .. }
            | Self::VecFilter {
                values,
                predicate: body,
                ..
            }
            | Self::OptionMap {
                option: values,
                body,
                ..
            }
            | Self::OptionFilter {
                option: values,
                predicate: body,
                ..
            } => vec![values, body],
            Self::VecAppend { values, value } => vec![values, value],
            Self::VecGetOr {
                values, default, ..
            }
            | Self::OptionUnwrapOr {
                option: values,
                default,
            } => vec![values, default],
            Self::MatchOption {
                option, some, none, ..
            } => vec![option, some, none],
            Self::ClosureCall { input, body, .. } => vec![input, body],
            Self::Index { values, .. } => vec![values],
            Self::I64(_)
            | Self::Bool(_)
            | Self::Text(_)
            | Self::F64(_)
            | Self::Variable { .. }
            | Self::None => Vec::new(),
        }
    }

    fn children(&self) -> Vec<&Self> {
        match self {
            Self::Add(left, right)
            | Self::Subtract(left, right)
            | Self::Multiply(left, right)
            | Self::Equal(left, right)
            | Self::Less(left, right)
            | Self::And(left, right)
            | Self::Or(left, right)
            | Self::Concat(left, right) => vec![left, right],
            Self::Not(value)
            | Self::Uppercase(value)
            | Self::FormatI64(value)
            | Self::DebugVec(value)
            | Self::VecReverse(value)
            | Self::VecLen(value)
            | Self::Some(value)
            | Self::OptionIsSome(value) => vec![value],
            Self::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => vec![condition, then_expr, else_expr],
            Self::Replace { value, .. } => vec![value],
            Self::VecLiteral(values) => values.iter().collect(),
            Self::VecMap { values, body, .. }
            | Self::VecFilter {
                values,
                predicate: body,
                ..
            }
            | Self::OptionMap {
                option: values,
                body,
                ..
            }
            | Self::OptionFilter {
                option: values,
                predicate: body,
                ..
            } => vec![values, body],
            Self::VecAppend { values, value } => vec![values, value],
            Self::VecGetOr {
                values, default, ..
            }
            | Self::OptionUnwrapOr {
                option: values,
                default,
            } => vec![values, default],
            Self::MatchOption {
                option, some, none, ..
            } => vec![option, some, none],
            Self::ClosureCall { input, body, .. } => vec![input, body],
            Self::RawAdd(left, right)
            | Self::RawSub(left, right)
            | Self::RawMul(left, right)
            | Self::RawDiv(left, right)
            | Self::RawRem(left, right)
            | Self::FAdd(left, right)
            | Self::FSub(left, right)
            | Self::FMul(left, right)
            | Self::FDiv(left, right)
            | Self::FLess(left, right)
            | Self::FEq(left, right)
            | Self::NarrowArith { left, right, .. } => vec![left, right],
            Self::Cast(value, _)
            | Self::Unwrap(value)
            | Self::I64ToF64(value)
            | Self::F64ToI64(value)
            | Self::FormatF64(value)
            | Self::DebugF64(value)
            | Self::FormatSpec { value, .. } => vec![value],
            Self::Index { values, .. } => vec![values],
            Self::I64(_)
            | Self::Bool(_)
            | Self::Text(_)
            | Self::F64(_)
            | Self::Variable { .. }
            | Self::None => Vec::new(),
        }
    }
}

/// The identity helper the raw arithmetic operands pass through. It is a plain
/// function, not a `const fn`, so the compiler cannot evaluate it while linting
/// and cannot reject a program for a constant overflow or divide by zero that
/// only shows up at runtime.
pub fn opaque_helper() -> &'static str {
    "fn diff_opaque(x: i64) -> i64 {\n    x\n}\n\n"
}
