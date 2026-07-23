use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum GeneratedType {
    I64,
    Bool,
    String,
    VecI64,
    OptionI64,
}

impl GeneratedType {
    pub fn rust(self) -> &'static str {
        match self {
            Self::I64 => "i64",
            Self::Bool => "bool",
            Self::String => "String",
            Self::VecI64 => "Vec<i64>",
            Self::OptionI64 => "Option<i64>",
        }
    }

    fn is_owned(self) -> bool {
        matches!(self, Self::String | Self::VecI64)
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
            | Self::OptionUnwrapOr { .. } => GeneratedType::I64,
            Self::Bool(_)
            | Self::Equal(..)
            | Self::Less(..)
            | Self::And(..)
            | Self::Or(..)
            | Self::Not(_)
            | Self::OptionIsSome(_) => GeneratedType::Bool,
            Self::Text(_)
            | Self::Concat(..)
            | Self::Uppercase(_)
            | Self::Replace { .. }
            | Self::FormatI64(_)
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

    pub fn render(&self) -> String {
        match self {
            Self::I64(value) => format!("{value}i64"),
            Self::Bool(value) => value.to_string(),
            Self::Text(value) => format!("{value:?}.to_string()"),
            Self::Variable { name, ty } if ty.is_owned() => format!("{name}.clone()"),
            Self::Variable { name, .. } => name.clone(),
            Self::Add(left, right) => {
                format!("{}.saturating_add({})", grouped(left), right.render())
            }
            Self::Subtract(left, right) => {
                format!("{}.saturating_sub({})", grouped(left), right.render())
            }
            Self::Multiply(left, right) => {
                format!("{}.saturating_mul({})", grouped(left), right.render())
            }
            Self::Equal(left, right) => format!("({} == {})", grouped(left), grouped(right)),
            Self::Less(left, right) => format!("({} < {})", grouped(left), grouped(right)),
            Self::And(left, right) => format!("({} && {})", left.render(), right.render()),
            Self::Or(left, right) => format!("({} || {})", left.render(), right.render()),
            Self::Not(value) => format!("!{}", grouped(value)),
            Self::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => format!(
                "if {} {{ {} }} else {{ {} }}",
                condition.render(),
                then_expr.render(),
                else_expr.render()
            ),
            Self::Concat(left, right) => {
                format!(
                    "format!(\"{{}}{{}}\", {}, {})",
                    left.render(),
                    right.render()
                )
            }
            Self::Uppercase(value) => format!("{}.to_uppercase()", grouped(value)),
            Self::Replace { value, from, to } => {
                format!("{}.replace({from:?}, {to:?})", grouped(value))
            }
            Self::FormatI64(value) => format!("format!(\"{{}}\", {})", value.render()),
            Self::DebugVec(value) => format!("format!(\"{{:?}}\", {})", value.render()),
            Self::VecLiteral(values) => {
                if values.is_empty() {
                    return "Vec::<i64>::new()".to_string();
                }
                let values = values
                    .iter()
                    .map(Self::render)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("vec![{values}]")
            }
            Self::VecMap {
                values,
                binding,
                body,
            } => format!(
                "{}.into_iter().map(|{binding}: i64| {}).collect::<Vec<i64>>()",
                grouped(values),
                body.render()
            ),
            Self::VecFilter {
                values,
                binding,
                predicate,
            } => format!(
                "{}.into_iter().filter(|generated_ref| {{ let {binding} = *generated_ref; {} }}).collect::<Vec<i64>>()",
                grouped(values),
                predicate.render()
            ),
            Self::VecReverse(values) => format!(
                "{{ let mut generated_values = {}; generated_values.reverse(); generated_values }}",
                values.render()
            ),
            Self::VecAppend { values, value } => format!(
                "{{ let mut generated_values = {}; generated_values.push({}); generated_values }}",
                values.render(),
                value.render()
            ),
            Self::VecLen(values) => format!("{}.len() as i64", grouped(values)),
            Self::VecGetOr {
                values,
                index,
                default,
            } => format!(
                "{}.get({index}usize).copied().unwrap_or({})",
                grouped(values),
                default.render()
            ),
            Self::Some(value) => format!("Some({})", value.render()),
            Self::None => "None::<i64>".to_string(),
            Self::OptionMap {
                option,
                binding,
                body,
            } => format!(
                "{}.map(|{binding}: i64| {})",
                grouped(option),
                body.render()
            ),
            Self::OptionFilter {
                option,
                binding,
                predicate,
            } => format!(
                "{}.filter(|generated_ref| {{ let {binding} = *generated_ref; {} }})",
                grouped(option),
                predicate.render()
            ),
            Self::OptionUnwrapOr { option, default } => {
                format!("{}.unwrap_or({})", grouped(option), default.render())
            }
            Self::OptionIsSome(option) => format!("{}.is_some()", grouped(option)),
            Self::MatchOption {
                option,
                binding,
                some,
                none,
                ..
            } => format!(
                "match {} {{ Some({binding}) => {}, None => {} }}",
                option.render(),
                some.render(),
                none.render()
            ),
            Self::ClosureCall {
                binding,
                input,
                body,
                ..
            } => format!("(|{binding}: i64| {})({})", body.render(), input.render()),
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
            Self::I64(_) | Self::Bool(_) | Self::Text(_) | Self::None => false,
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
            Self::I64(_) | Self::Bool(_) | Self::Text(_) | Self::Variable { .. } | Self::None => {
                Vec::new()
            }
        }
    }
}

fn grouped(expr: &GeneratedExpr) -> String {
    format!("({})", expr.render())
}
