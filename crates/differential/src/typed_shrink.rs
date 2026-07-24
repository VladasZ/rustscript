use crate::typed::{GeneratedExpr, GeneratedType};

pub fn shrink(expression: &GeneratedExpr) -> Vec<GeneratedExpr> {
    let mut candidates = Vec::new();
    let minimal = minimal(expression.ty());
    if expression != &minimal {
        candidates.push(minimal);
    }
    add_direct_reductions(expression, &mut candidates);
    add_child_reductions(expression, &mut candidates);
    candidates
}

fn add_direct_reductions(expression: &GeneratedExpr, candidates: &mut Vec<GeneratedExpr>) {
    match expression {
        GeneratedExpr::Add(left, right)
        | GeneratedExpr::Subtract(left, right)
        | GeneratedExpr::Multiply(left, right)
        | GeneratedExpr::Equal(left, right)
        | GeneratedExpr::Less(left, right)
        | GeneratedExpr::And(left, right)
        | GeneratedExpr::Or(left, right)
        | GeneratedExpr::Concat(left, right)
        | GeneratedExpr::RawAdd(left, right)
        | GeneratedExpr::RawSub(left, right)
        | GeneratedExpr::RawMul(left, right)
        | GeneratedExpr::RawDiv(left, right)
        | GeneratedExpr::RawRem(left, right)
        | GeneratedExpr::FAdd(left, right)
        | GeneratedExpr::FSub(left, right)
        | GeneratedExpr::FMul(left, right)
        | GeneratedExpr::FDiv(left, right)
        | GeneratedExpr::FLess(left, right)
        | GeneratedExpr::FEq(left, right)
        | GeneratedExpr::NarrowArith { left, right, .. } => {
            push_same_type(candidates, expression.ty(), left);
            push_same_type(candidates, expression.ty(), right);
        }
        GeneratedExpr::Cast(value, _) => candidates.push((**value).clone()),
        GeneratedExpr::FormatSpec { value, .. } => {
            push_same_type(candidates, expression.ty(), value);
        }
        GeneratedExpr::If {
            then_expr,
            else_expr,
            ..
        } => {
            candidates.push((**then_expr).clone());
            candidates.push((**else_expr).clone());
        }
        GeneratedExpr::VecMap { values, .. }
        | GeneratedExpr::VecFilter { values, .. }
        | GeneratedExpr::VecReverse(values)
        | GeneratedExpr::VecAppend { values, .. } => candidates.push((**values).clone()),
        GeneratedExpr::OptionMap { option, .. } | GeneratedExpr::OptionFilter { option, .. } => {
            candidates.push((**option).clone());
        }
        GeneratedExpr::MatchOption { some, none, .. } => {
            candidates.push((**some).clone());
            candidates.push((**none).clone());
        }
        GeneratedExpr::ClosureCall { body, .. } => candidates.push((**body).clone()),
        GeneratedExpr::I64(_)
        | GeneratedExpr::Bool(_)
        | GeneratedExpr::Text(_)
        | GeneratedExpr::F64(_)
        | GeneratedExpr::Variable { .. }
        | GeneratedExpr::Not(_)
        | GeneratedExpr::Uppercase(_)
        | GeneratedExpr::Replace { .. }
        | GeneratedExpr::FormatI64(_)
        | GeneratedExpr::FormatF64(_)
        | GeneratedExpr::DebugF64(_)
        | GeneratedExpr::I64ToF64(_)
        | GeneratedExpr::F64ToI64(_)
        | GeneratedExpr::DebugVec(_)
        | GeneratedExpr::VecLiteral(_)
        | GeneratedExpr::VecLen(_)
        | GeneratedExpr::VecGetOr { .. }
        | GeneratedExpr::Some(_)
        | GeneratedExpr::None
        | GeneratedExpr::OptionUnwrapOr { .. }
        | GeneratedExpr::OptionIsSome(_)
        | GeneratedExpr::Index { .. }
        | GeneratedExpr::Unwrap(_) => {}
    }
}

fn add_child_reductions(expression: &GeneratedExpr, candidates: &mut Vec<GeneratedExpr>) {
    match expression {
        GeneratedExpr::Add(left, right) => {
            shrink_binary(candidates, left, right, GeneratedExpr::Add);
        }
        GeneratedExpr::Subtract(left, right) => {
            shrink_binary(candidates, left, right, GeneratedExpr::Subtract);
        }
        GeneratedExpr::Multiply(left, right) => {
            shrink_binary(candidates, left, right, GeneratedExpr::Multiply);
        }
        GeneratedExpr::Equal(left, right) => {
            shrink_binary(candidates, left, right, GeneratedExpr::Equal);
        }
        GeneratedExpr::Less(left, right) => {
            shrink_binary(candidates, left, right, GeneratedExpr::Less);
        }
        GeneratedExpr::And(left, right) => {
            shrink_binary(candidates, left, right, GeneratedExpr::And);
        }
        GeneratedExpr::Or(left, right) => {
            shrink_binary(candidates, left, right, GeneratedExpr::Or);
        }
        GeneratedExpr::Concat(left, right) => {
            shrink_binary(candidates, left, right, GeneratedExpr::Concat);
        }
        GeneratedExpr::Not(value) => {
            shrink_unary(candidates, value, GeneratedExpr::Not);
        }
        GeneratedExpr::If {
            condition,
            then_expr,
            else_expr,
            ty,
        } => {
            for shrunk in condition.shrinks() {
                candidates.push(GeneratedExpr::If {
                    condition: Box::new(shrunk),
                    then_expr: then_expr.clone(),
                    else_expr: else_expr.clone(),
                    ty: *ty,
                });
            }
            for shrunk in then_expr.shrinks() {
                candidates.push(GeneratedExpr::If {
                    condition: condition.clone(),
                    then_expr: Box::new(shrunk),
                    else_expr: else_expr.clone(),
                    ty: *ty,
                });
            }
            for shrunk in else_expr.shrinks() {
                candidates.push(GeneratedExpr::If {
                    condition: condition.clone(),
                    then_expr: then_expr.clone(),
                    else_expr: Box::new(shrunk),
                    ty: *ty,
                });
            }
        }
        GeneratedExpr::Uppercase(value) => {
            shrink_unary(candidates, value, GeneratedExpr::Uppercase);
        }
        GeneratedExpr::Replace { value, from, to } => {
            for shrunk in value.shrinks() {
                candidates.push(GeneratedExpr::Replace {
                    value: Box::new(shrunk),
                    from: from.clone(),
                    to: to.clone(),
                });
            }
        }
        GeneratedExpr::FormatI64(value) => {
            shrink_unary(candidates, value, GeneratedExpr::FormatI64);
        }
        GeneratedExpr::DebugVec(value) => {
            shrink_unary(candidates, value, GeneratedExpr::DebugVec);
        }
        GeneratedExpr::VecLiteral(values) => {
            if !values.is_empty() {
                let mut shorter = values.clone();
                shorter.pop();
                candidates.push(GeneratedExpr::VecLiteral(shorter));
            }
            for (index, value) in values.iter().enumerate() {
                for shrunk in value.shrinks() {
                    let mut changed = values.clone();
                    changed[index] = shrunk;
                    candidates.push(GeneratedExpr::VecLiteral(changed));
                }
            }
        }
        GeneratedExpr::VecMap {
            values,
            binding,
            body,
        } => {
            for shrunk in values.shrinks() {
                candidates.push(GeneratedExpr::VecMap {
                    values: Box::new(shrunk),
                    binding: binding.clone(),
                    body: body.clone(),
                });
            }
            for shrunk in body.shrinks() {
                candidates.push(GeneratedExpr::VecMap {
                    values: values.clone(),
                    binding: binding.clone(),
                    body: Box::new(shrunk),
                });
            }
        }
        GeneratedExpr::VecFilter {
            values,
            binding,
            predicate,
        } => {
            for shrunk in values.shrinks() {
                candidates.push(GeneratedExpr::VecFilter {
                    values: Box::new(shrunk),
                    binding: binding.clone(),
                    predicate: predicate.clone(),
                });
            }
            for shrunk in predicate.shrinks() {
                candidates.push(GeneratedExpr::VecFilter {
                    values: values.clone(),
                    binding: binding.clone(),
                    predicate: Box::new(shrunk),
                });
            }
        }
        GeneratedExpr::VecReverse(value) => {
            shrink_unary(candidates, value, GeneratedExpr::VecReverse);
        }
        GeneratedExpr::VecAppend { values, value } => {
            for shrunk in values.shrinks() {
                candidates.push(GeneratedExpr::VecAppend {
                    values: Box::new(shrunk),
                    value: value.clone(),
                });
            }
            for shrunk in value.shrinks() {
                candidates.push(GeneratedExpr::VecAppend {
                    values: values.clone(),
                    value: Box::new(shrunk),
                });
            }
        }
        GeneratedExpr::VecLen(value) => {
            shrink_unary(candidates, value, GeneratedExpr::VecLen);
        }
        GeneratedExpr::VecGetOr {
            values,
            index,
            default,
        } => {
            for shrunk in values.shrinks() {
                candidates.push(GeneratedExpr::VecGetOr {
                    values: Box::new(shrunk),
                    index: *index,
                    default: default.clone(),
                });
            }
            for shrunk in default.shrinks() {
                candidates.push(GeneratedExpr::VecGetOr {
                    values: values.clone(),
                    index: *index,
                    default: Box::new(shrunk),
                });
            }
        }
        GeneratedExpr::Some(value) => {
            shrink_unary(candidates, value, GeneratedExpr::Some);
        }
        GeneratedExpr::OptionMap {
            option,
            binding,
            body,
        } => {
            for shrunk in option.shrinks() {
                candidates.push(GeneratedExpr::OptionMap {
                    option: Box::new(shrunk),
                    binding: binding.clone(),
                    body: body.clone(),
                });
            }
            for shrunk in body.shrinks() {
                candidates.push(GeneratedExpr::OptionMap {
                    option: option.clone(),
                    binding: binding.clone(),
                    body: Box::new(shrunk),
                });
            }
        }
        GeneratedExpr::OptionFilter {
            option,
            binding,
            predicate,
        } => {
            for shrunk in option.shrinks() {
                candidates.push(GeneratedExpr::OptionFilter {
                    option: Box::new(shrunk),
                    binding: binding.clone(),
                    predicate: predicate.clone(),
                });
            }
            for shrunk in predicate.shrinks() {
                candidates.push(GeneratedExpr::OptionFilter {
                    option: option.clone(),
                    binding: binding.clone(),
                    predicate: Box::new(shrunk),
                });
            }
        }
        GeneratedExpr::OptionUnwrapOr { option, default } => {
            for shrunk in option.shrinks() {
                candidates.push(GeneratedExpr::OptionUnwrapOr {
                    option: Box::new(shrunk),
                    default: default.clone(),
                });
            }
            for shrunk in default.shrinks() {
                candidates.push(GeneratedExpr::OptionUnwrapOr {
                    option: option.clone(),
                    default: Box::new(shrunk),
                });
            }
        }
        GeneratedExpr::OptionIsSome(value) => {
            shrink_unary(candidates, value, GeneratedExpr::OptionIsSome);
        }
        GeneratedExpr::MatchOption {
            option,
            binding,
            some,
            none,
            ty,
        } => {
            for shrunk in option.shrinks() {
                candidates.push(GeneratedExpr::MatchOption {
                    option: Box::new(shrunk),
                    binding: binding.clone(),
                    some: some.clone(),
                    none: none.clone(),
                    ty: *ty,
                });
            }
            for shrunk in some.shrinks() {
                candidates.push(GeneratedExpr::MatchOption {
                    option: option.clone(),
                    binding: binding.clone(),
                    some: Box::new(shrunk),
                    none: none.clone(),
                    ty: *ty,
                });
            }
            for shrunk in none.shrinks() {
                candidates.push(GeneratedExpr::MatchOption {
                    option: option.clone(),
                    binding: binding.clone(),
                    some: some.clone(),
                    none: Box::new(shrunk),
                    ty: *ty,
                });
            }
        }
        GeneratedExpr::ClosureCall {
            binding,
            input,
            body,
            ty,
        } => {
            for shrunk in input.shrinks() {
                candidates.push(GeneratedExpr::ClosureCall {
                    binding: binding.clone(),
                    input: Box::new(shrunk),
                    body: body.clone(),
                    ty: *ty,
                });
            }
            for shrunk in body.shrinks() {
                candidates.push(GeneratedExpr::ClosureCall {
                    binding: binding.clone(),
                    input: input.clone(),
                    body: Box::new(shrunk),
                    ty: *ty,
                });
            }
        }
        GeneratedExpr::RawAdd(left, right) => {
            shrink_binary(candidates, left, right, GeneratedExpr::RawAdd);
        }
        GeneratedExpr::RawSub(left, right) => {
            shrink_binary(candidates, left, right, GeneratedExpr::RawSub);
        }
        GeneratedExpr::RawMul(left, right) => {
            shrink_binary(candidates, left, right, GeneratedExpr::RawMul);
        }
        GeneratedExpr::RawDiv(left, right) => {
            shrink_binary(candidates, left, right, GeneratedExpr::RawDiv);
        }
        GeneratedExpr::RawRem(left, right) => {
            shrink_binary(candidates, left, right, GeneratedExpr::RawRem);
        }
        GeneratedExpr::FAdd(left, right) => {
            shrink_binary(candidates, left, right, GeneratedExpr::FAdd);
        }
        GeneratedExpr::FSub(left, right) => {
            shrink_binary(candidates, left, right, GeneratedExpr::FSub);
        }
        GeneratedExpr::FMul(left, right) => {
            shrink_binary(candidates, left, right, GeneratedExpr::FMul);
        }
        GeneratedExpr::FDiv(left, right) => {
            shrink_binary(candidates, left, right, GeneratedExpr::FDiv);
        }
        GeneratedExpr::FLess(left, right) => {
            shrink_binary(candidates, left, right, GeneratedExpr::FLess);
        }
        GeneratedExpr::FEq(left, right) => {
            shrink_binary(candidates, left, right, GeneratedExpr::FEq);
        }
        GeneratedExpr::NarrowArith {
            target,
            op,
            left,
            right,
        } => {
            for shrunk in left.shrinks() {
                candidates.push(GeneratedExpr::NarrowArith {
                    target: *target,
                    op: *op,
                    left: Box::new(shrunk),
                    right: right.clone(),
                });
            }
            for shrunk in right.shrinks() {
                candidates.push(GeneratedExpr::NarrowArith {
                    target: *target,
                    op: *op,
                    left: left.clone(),
                    right: Box::new(shrunk),
                });
            }
        }
        GeneratedExpr::Cast(value, target) => {
            for shrunk in value.shrinks() {
                candidates.push(GeneratedExpr::Cast(Box::new(shrunk), *target));
            }
        }
        GeneratedExpr::I64ToF64(value) => {
            shrink_unary(candidates, value, GeneratedExpr::I64ToF64);
        }
        GeneratedExpr::F64ToI64(value) => {
            shrink_unary(candidates, value, GeneratedExpr::F64ToI64);
        }
        GeneratedExpr::FormatF64(value) => {
            shrink_unary(candidates, value, GeneratedExpr::FormatF64);
        }
        GeneratedExpr::DebugF64(value) => {
            shrink_unary(candidates, value, GeneratedExpr::DebugF64);
        }
        GeneratedExpr::FormatSpec { spec, value } => {
            for shrunk in value.shrinks() {
                candidates.push(GeneratedExpr::FormatSpec {
                    spec: spec.clone(),
                    value: Box::new(shrunk),
                });
            }
        }
        GeneratedExpr::Index { values, index } => {
            for shrunk in values.shrinks() {
                candidates.push(GeneratedExpr::Index {
                    values: Box::new(shrunk),
                    index: *index,
                });
            }
        }
        GeneratedExpr::Unwrap(value) => {
            shrink_unary(candidates, value, GeneratedExpr::Unwrap);
        }
        GeneratedExpr::I64(_)
        | GeneratedExpr::Bool(_)
        | GeneratedExpr::Text(_)
        | GeneratedExpr::F64(_)
        | GeneratedExpr::Variable { .. }
        | GeneratedExpr::None => {}
    }
}

fn shrink_binary(
    candidates: &mut Vec<GeneratedExpr>,
    left: &GeneratedExpr,
    right: &GeneratedExpr,
    construct: fn(Box<GeneratedExpr>, Box<GeneratedExpr>) -> GeneratedExpr,
) {
    for shrunk in left.shrinks() {
        candidates.push(construct(Box::new(shrunk), Box::new(right.clone())));
    }
    for shrunk in right.shrinks() {
        candidates.push(construct(Box::new(left.clone()), Box::new(shrunk)));
    }
}

fn shrink_unary(
    candidates: &mut Vec<GeneratedExpr>,
    value: &GeneratedExpr,
    construct: fn(Box<GeneratedExpr>) -> GeneratedExpr,
) {
    for shrunk in value.shrinks() {
        candidates.push(construct(Box::new(shrunk)));
    }
}

fn minimal(ty: GeneratedType) -> GeneratedExpr {
    match ty {
        GeneratedType::I64 => GeneratedExpr::I64(0),
        GeneratedType::F64 => GeneratedExpr::F64("0.0".to_string()),
        GeneratedType::Bool => GeneratedExpr::Bool(false),
        GeneratedType::String => GeneratedExpr::Text(String::new()),
        GeneratedType::VecI64 => GeneratedExpr::VecLiteral(Vec::new()),
        GeneratedType::OptionI64 => GeneratedExpr::None,
    }
}

fn push_same_type(
    candidates: &mut Vec<GeneratedExpr>,
    ty: GeneratedType,
    expression: &GeneratedExpr,
) {
    if expression.ty() == ty {
        candidates.push(expression.clone());
    }
}
