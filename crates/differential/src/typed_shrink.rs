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
        | GeneratedExpr::FEq(left, right) => {
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
    if let Some((construct, left, right)) = binary_parts(expression) {
        shrink_binary(candidates, left, right, construct);
        return;
    }
    if let Some((construct, value)) = unary_parts(expression) {
        shrink_unary(candidates, value, construct);
        return;
    }
    if vec_child_reductions(expression, candidates) {
        return;
    }
    if option_child_reductions(expression, candidates) {
        return;
    }
    match expression {
        GeneratedExpr::If { .. } => if_child_reductions(expression, candidates),
        GeneratedExpr::Replace { value, from, to } => {
            for shrunk in value.shrinks() {
                candidates.push(GeneratedExpr::Replace {
                    value: Box::new(shrunk),
                    from: from.clone(),
                    to: to.clone(),
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
        GeneratedExpr::Cast(value, target) => {
            for shrunk in value.shrinks() {
                candidates.push(GeneratedExpr::Cast(Box::new(shrunk), *target));
            }
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
        _ => {}
    }
}

/// Child reductions of an `if`, one branch shrunk at a time.
fn if_child_reductions(expression: &GeneratedExpr, candidates: &mut Vec<GeneratedExpr>) {
    let GeneratedExpr::If {
        condition,
        then_expr,
        else_expr,
        ty,
    } = expression
    else {
        unreachable!()
    };
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

type BinaryCtor = fn(Box<GeneratedExpr>, Box<GeneratedExpr>) -> GeneratedExpr;
type UnaryCtor = fn(Box<GeneratedExpr>) -> GeneratedExpr;

/// The constructor and children of a two-child tuple variant, so every binary
/// operator shrinks through one code path.
fn binary_parts(e: &GeneratedExpr) -> Option<(BinaryCtor, &GeneratedExpr, &GeneratedExpr)> {
    use GeneratedExpr as G;
    let (construct, left, right): (BinaryCtor, _, _) = match e {
        G::Add(l, r) => (G::Add, &**l, &**r),
        G::Subtract(l, r) => (G::Subtract, &**l, &**r),
        G::Multiply(l, r) => (G::Multiply, &**l, &**r),
        G::Equal(l, r) => (G::Equal, &**l, &**r),
        G::Less(l, r) => (G::Less, &**l, &**r),
        G::And(l, r) => (G::And, &**l, &**r),
        G::Or(l, r) => (G::Or, &**l, &**r),
        G::Concat(l, r) => (G::Concat, &**l, &**r),
        G::RawAdd(l, r) => (G::RawAdd, &**l, &**r),
        G::RawSub(l, r) => (G::RawSub, &**l, &**r),
        G::RawMul(l, r) => (G::RawMul, &**l, &**r),
        G::RawDiv(l, r) => (G::RawDiv, &**l, &**r),
        G::RawRem(l, r) => (G::RawRem, &**l, &**r),
        G::FAdd(l, r) => (G::FAdd, &**l, &**r),
        G::FSub(l, r) => (G::FSub, &**l, &**r),
        G::FMul(l, r) => (G::FMul, &**l, &**r),
        G::FDiv(l, r) => (G::FDiv, &**l, &**r),
        G::FLess(l, r) => (G::FLess, &**l, &**r),
        G::FEq(l, r) => (G::FEq, &**l, &**r),
        _ => return None,
    };
    Some((construct, left, right))
}

/// The constructor and child of a one-child tuple variant.
fn unary_parts(e: &GeneratedExpr) -> Option<(UnaryCtor, &GeneratedExpr)> {
    use GeneratedExpr as G;
    let (construct, value): (UnaryCtor, _) = match e {
        G::Not(v) => (G::Not, &**v),
        G::Uppercase(v) => (G::Uppercase, &**v),
        G::FormatI64(v) => (G::FormatI64, &**v),
        G::DebugVec(v) => (G::DebugVec, &**v),
        G::VecReverse(v) => (G::VecReverse, &**v),
        G::VecLen(v) => (G::VecLen, &**v),
        G::Some(v) => (G::Some, &**v),
        G::OptionIsSome(v) => (G::OptionIsSome, &**v),
        G::I64ToF64(v) => (G::I64ToF64, &**v),
        G::F64ToI64(v) => (G::F64ToI64, &**v),
        G::FormatF64(v) => (G::FormatF64, &**v),
        G::DebugF64(v) => (G::DebugF64, &**v),
        G::Unwrap(v) => (G::Unwrap, &**v),
        _ => return None,
    };
    Some((construct, value))
}

/// Child reductions of the vec-shaped variants. True when the variant was one
/// of them.
fn vec_child_reductions(expression: &GeneratedExpr, candidates: &mut Vec<GeneratedExpr>) -> bool {
    match expression {
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
        _ => return false,
    }
    true
}

/// Child reductions of the option-shaped variants. True when the variant was
/// one of them.
fn option_child_reductions(
    expression: &GeneratedExpr,
    candidates: &mut Vec<GeneratedExpr>,
) -> bool {
    match expression {
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
        _ => return false,
    }
    true
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
