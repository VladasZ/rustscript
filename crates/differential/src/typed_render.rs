use crate::typed::GeneratedExpr;

impl GeneratedExpr {
    pub fn render(&self) -> String {
        match self {
            // `-9223372036854775808i64` overflows the literal and will not
            // parse, so the minimum is written through its associated constant.
            Self::I64(i64::MIN) => "i64::MIN".to_string(),
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
            Self::Cast(value, target) => {
                format!("(({} as {}) as i64)", value.render(), target.rust())
            }
            // Each operand goes through `diff_opaque`, a plain non-const
            // function, so the compiler cannot fold the operation to a constant
            // and reject it with the const-overflow lint. The overflow, divide
            // by zero, or `i64::MIN / -1` then still happens at runtime.
            Self::RawAdd(left, right) => raw_binary_source(left, "+", right),
            Self::RawSub(left, right) => raw_binary_source(left, "-", right),
            Self::RawMul(left, right) => raw_binary_source(left, "*", right),
            Self::RawDiv(left, right) => raw_binary_source(left, "/", right),
            Self::RawRem(left, right) => raw_binary_source(left, "%", right),
            Self::Index { values, index } => format!("{}[{index}usize]", grouped(values)),
            Self::Unwrap(value) => format!("{}.unwrap()", grouped(value)),
            Self::F64(token) => {
                if token.contains("::") {
                    token.clone()
                } else {
                    format!("{token}f64")
                }
            }
            Self::FAdd(left, right) => format!("({} + {})", left.render(), right.render()),
            Self::FSub(left, right) => format!("({} - {})", left.render(), right.render()),
            Self::FMul(left, right) => format!("({} * {})", left.render(), right.render()),
            Self::FDiv(left, right) => format!("({} / {})", left.render(), right.render()),
            Self::FLess(left, right) => format!("({} < {})", grouped(left), grouped(right)),
            Self::FEq(left, right) => format!("({} == {})", grouped(left), grouped(right)),
            Self::I64ToF64(value) => format!("({} as f64)", grouped(value)),
            Self::F64ToI64(value) => format!("({} as i64)", grouped(value)),
            Self::FormatF64(value) => format!("format!(\"{{}}\", {})", value.render()),
            Self::DebugF64(value) => format!("format!(\"{{:?}}\", {})", value.render()),
            Self::FormatSpec { spec, value } => {
                format!("format!(\"{{{spec}}}\", {})", value.render())
            }
        }
    }
}

fn grouped(expr: &GeneratedExpr) -> String {
    format!("({})", expr.render())
}

fn raw_binary_source(left: &GeneratedExpr, op: &str, right: &GeneratedExpr) -> String {
    format!(
        "(diff_opaque({}) {op} diff_opaque({}))",
        left.render(),
        right.render()
    )
}
