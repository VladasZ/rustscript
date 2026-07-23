use rand::RngExt;
use rand::rngs::StdRng;

use crate::typed::{GeneratedExpr, GeneratedType};

#[derive(Clone)]
pub struct TypedBinding {
    pub name: String,
    pub ty: GeneratedType,
}

pub fn expression(
    ty: GeneratedType,
    depth: usize,
    bindings: &[TypedBinding],
    rng: &mut StdRng,
    next_name: &mut usize,
) -> GeneratedExpr {
    if depth == 0 {
        return leaf(ty, bindings, rng);
    }
    match ty {
        GeneratedType::I64 => i64_expression(depth, bindings, rng, next_name),
        GeneratedType::Bool => bool_expression(depth, bindings, rng, next_name),
        GeneratedType::String => string_expression(depth, bindings, rng, next_name),
        GeneratedType::VecI64 => vec_expression(depth, bindings, rng, next_name),
        GeneratedType::OptionI64 => option_expression(depth, bindings, rng, next_name),
    }
}

fn i64_expression(
    depth: usize,
    bindings: &[TypedBinding],
    rng: &mut StdRng,
    next_name: &mut usize,
) -> GeneratedExpr {
    let child = depth - 1;
    match rng.random_range(0..11) {
        0 => leaf(GeneratedType::I64, bindings, rng),
        1 => GeneratedExpr::Add(
            boxed(GeneratedType::I64, child, bindings, rng, next_name),
            boxed(GeneratedType::I64, child, bindings, rng, next_name),
        ),
        2 => GeneratedExpr::Subtract(
            boxed(GeneratedType::I64, child, bindings, rng, next_name),
            boxed(GeneratedType::I64, child, bindings, rng, next_name),
        ),
        3 => GeneratedExpr::Multiply(
            boxed(GeneratedType::I64, child, bindings, rng, next_name),
            boxed(GeneratedType::I64, child, bindings, rng, next_name),
        ),
        4 => if_expression(GeneratedType::I64, child, bindings, rng, next_name),
        5 => GeneratedExpr::VecLen(boxed(
            GeneratedType::VecI64,
            child,
            bindings,
            rng,
            next_name,
        )),
        6 => GeneratedExpr::VecGetOr {
            values: boxed(GeneratedType::VecI64, child, bindings, rng, next_name),
            index: rng.random_range(0..=6),
            default: boxed(GeneratedType::I64, child, bindings, rng, next_name),
        },
        7 => GeneratedExpr::OptionUnwrapOr {
            option: boxed(GeneratedType::OptionI64, child, bindings, rng, next_name),
            default: boxed(GeneratedType::I64, child, bindings, rng, next_name),
        },
        8 => match_option(GeneratedType::I64, child, bindings, rng, next_name),
        9 => closure_call(GeneratedType::I64, child, bindings, rng, next_name),
        _ => {
            let value = boxed(GeneratedType::I64, child, bindings, rng, next_name);
            let option = if rng.random_bool(0.7) {
                GeneratedExpr::Some(value)
            } else {
                GeneratedExpr::None
            };
            GeneratedExpr::OptionUnwrapOr {
                option: Box::new(option),
                default: Box::new(GeneratedExpr::I64(rng.random_range(-20..=20))),
            }
        }
    }
}

fn bool_expression(
    depth: usize,
    bindings: &[TypedBinding],
    rng: &mut StdRng,
    next_name: &mut usize,
) -> GeneratedExpr {
    let child = depth - 1;
    match rng.random_range(0..10) {
        0 => leaf(GeneratedType::Bool, bindings, rng),
        1 => GeneratedExpr::Equal(
            boxed(GeneratedType::I64, child, bindings, rng, next_name),
            boxed(GeneratedType::I64, child, bindings, rng, next_name),
        ),
        2 => GeneratedExpr::Less(
            boxed(GeneratedType::I64, child, bindings, rng, next_name),
            boxed(GeneratedType::I64, child, bindings, rng, next_name),
        ),
        3 => GeneratedExpr::And(
            boxed(GeneratedType::Bool, child, bindings, rng, next_name),
            boxed(GeneratedType::Bool, child, bindings, rng, next_name),
        ),
        4 => GeneratedExpr::Or(
            boxed(GeneratedType::Bool, child, bindings, rng, next_name),
            boxed(GeneratedType::Bool, child, bindings, rng, next_name),
        ),
        5 => GeneratedExpr::Not(boxed(GeneratedType::Bool, child, bindings, rng, next_name)),
        6 => if_expression(GeneratedType::Bool, child, bindings, rng, next_name),
        7 => GeneratedExpr::OptionIsSome(boxed(
            GeneratedType::OptionI64,
            child,
            bindings,
            rng,
            next_name,
        )),
        8 => match_option(GeneratedType::Bool, child, bindings, rng, next_name),
        _ => closure_call(GeneratedType::Bool, child, bindings, rng, next_name),
    }
}

fn string_expression(
    depth: usize,
    bindings: &[TypedBinding],
    rng: &mut StdRng,
    next_name: &mut usize,
) -> GeneratedExpr {
    let child = depth - 1;
    match rng.random_range(0..10) {
        0 => leaf(GeneratedType::String, bindings, rng),
        1 => GeneratedExpr::Concat(
            boxed(GeneratedType::String, child, bindings, rng, next_name),
            boxed(GeneratedType::String, child, bindings, rng, next_name),
        ),
        2 => GeneratedExpr::Uppercase(boxed(
            GeneratedType::String,
            child,
            bindings,
            rng,
            next_name,
        )),
        3 => GeneratedExpr::Replace {
            value: boxed(GeneratedType::String, child, bindings, rng, next_name),
            from: word(rng).to_string(),
            to: word(rng).to_string(),
        },
        4 => GeneratedExpr::FormatI64(boxed(GeneratedType::I64, child, bindings, rng, next_name)),
        5 => GeneratedExpr::DebugVec(boxed(
            GeneratedType::VecI64,
            child,
            bindings,
            rng,
            next_name,
        )),
        6 => if_expression(GeneratedType::String, child, bindings, rng, next_name),
        7 => match_option(GeneratedType::String, child, bindings, rng, next_name),
        8 => closure_call(GeneratedType::String, child, bindings, rng, next_name),
        _ => GeneratedExpr::Concat(
            Box::new(GeneratedExpr::Text(word(rng).to_string())),
            Box::new(GeneratedExpr::FormatI64(boxed(
                GeneratedType::I64,
                child,
                bindings,
                rng,
                next_name,
            ))),
        ),
    }
}

fn vec_expression(
    depth: usize,
    bindings: &[TypedBinding],
    rng: &mut StdRng,
    next_name: &mut usize,
) -> GeneratedExpr {
    let child = depth - 1;
    match rng.random_range(0..8) {
        0 => leaf(GeneratedType::VecI64, bindings, rng),
        1 => {
            let binding = fresh_name("map_value", next_name);
            let mut nested = bindings.to_vec();
            nested.push(TypedBinding {
                name: binding.clone(),
                ty: GeneratedType::I64,
            });
            GeneratedExpr::VecMap {
                values: boxed(GeneratedType::VecI64, child, bindings, rng, next_name),
                binding,
                body: boxed(GeneratedType::I64, child, &nested, rng, next_name),
            }
        }
        2 => {
            let binding = fresh_name("filter_value", next_name);
            let mut nested = bindings.to_vec();
            nested.push(TypedBinding {
                name: binding.clone(),
                ty: GeneratedType::I64,
            });
            GeneratedExpr::VecFilter {
                values: boxed(GeneratedType::VecI64, child, bindings, rng, next_name),
                binding,
                predicate: boxed(GeneratedType::Bool, child, &nested, rng, next_name),
            }
        }
        3 => GeneratedExpr::VecReverse(boxed(
            GeneratedType::VecI64,
            child,
            bindings,
            rng,
            next_name,
        )),
        4 => GeneratedExpr::VecAppend {
            values: boxed(GeneratedType::VecI64, child, bindings, rng, next_name),
            value: boxed(GeneratedType::I64, child, bindings, rng, next_name),
        },
        5 => if_expression(GeneratedType::VecI64, child, bindings, rng, next_name),
        6 => closure_call(GeneratedType::VecI64, child, bindings, rng, next_name),
        _ => vec_literal(child, bindings, rng, next_name),
    }
}

fn option_expression(
    depth: usize,
    bindings: &[TypedBinding],
    rng: &mut StdRng,
    next_name: &mut usize,
) -> GeneratedExpr {
    let child = depth - 1;
    match rng.random_range(0..8) {
        0 => leaf(GeneratedType::OptionI64, bindings, rng),
        1 => GeneratedExpr::Some(boxed(GeneratedType::I64, child, bindings, rng, next_name)),
        2 => GeneratedExpr::None,
        3 => {
            let binding = fresh_name("option_map", next_name);
            let mut nested = bindings.to_vec();
            nested.push(TypedBinding {
                name: binding.clone(),
                ty: GeneratedType::I64,
            });
            GeneratedExpr::OptionMap {
                option: boxed(GeneratedType::OptionI64, child, bindings, rng, next_name),
                binding,
                body: boxed(GeneratedType::I64, child, &nested, rng, next_name),
            }
        }
        4 => {
            let binding = fresh_name("option_filter", next_name);
            let mut nested = bindings.to_vec();
            nested.push(TypedBinding {
                name: binding.clone(),
                ty: GeneratedType::I64,
            });
            GeneratedExpr::OptionFilter {
                option: boxed(GeneratedType::OptionI64, child, bindings, rng, next_name),
                binding,
                predicate: boxed(GeneratedType::Bool, child, &nested, rng, next_name),
            }
        }
        5 => if_expression(GeneratedType::OptionI64, child, bindings, rng, next_name),
        6 => closure_call(GeneratedType::OptionI64, child, bindings, rng, next_name),
        _ => GeneratedExpr::Some(Box::new(GeneratedExpr::ClosureCall {
            binding: fresh_name("option_closure", next_name),
            input: boxed(GeneratedType::I64, child, bindings, rng, next_name),
            body: boxed(GeneratedType::I64, child, bindings, rng, next_name),
            ty: GeneratedType::I64,
        })),
    }
}

fn if_expression(
    ty: GeneratedType,
    depth: usize,
    bindings: &[TypedBinding],
    rng: &mut StdRng,
    next_name: &mut usize,
) -> GeneratedExpr {
    GeneratedExpr::If {
        condition: boxed(GeneratedType::Bool, depth, bindings, rng, next_name),
        then_expr: boxed(ty, depth, bindings, rng, next_name),
        else_expr: boxed(ty, depth, bindings, rng, next_name),
        ty,
    }
}

fn match_option(
    ty: GeneratedType,
    depth: usize,
    bindings: &[TypedBinding],
    rng: &mut StdRng,
    next_name: &mut usize,
) -> GeneratedExpr {
    let binding = fresh_name("matched", next_name);
    let mut nested = bindings.to_vec();
    nested.push(TypedBinding {
        name: binding.clone(),
        ty: GeneratedType::I64,
    });
    GeneratedExpr::MatchOption {
        option: boxed(GeneratedType::OptionI64, depth, bindings, rng, next_name),
        binding,
        some: boxed(ty, depth, &nested, rng, next_name),
        none: boxed(ty, depth, bindings, rng, next_name),
        ty,
    }
}

fn closure_call(
    ty: GeneratedType,
    depth: usize,
    bindings: &[TypedBinding],
    rng: &mut StdRng,
    next_name: &mut usize,
) -> GeneratedExpr {
    let binding = fresh_name("closure_arg", next_name);
    let mut nested = bindings.to_vec();
    nested.push(TypedBinding {
        name: binding.clone(),
        ty: GeneratedType::I64,
    });
    GeneratedExpr::ClosureCall {
        binding,
        input: boxed(GeneratedType::I64, depth, bindings, rng, next_name),
        body: boxed(ty, depth, &nested, rng, next_name),
        ty,
    }
}

fn boxed(
    ty: GeneratedType,
    depth: usize,
    bindings: &[TypedBinding],
    rng: &mut StdRng,
    next_name: &mut usize,
) -> Box<GeneratedExpr> {
    Box::new(expression(ty, depth, bindings, rng, next_name))
}

fn leaf(ty: GeneratedType, bindings: &[TypedBinding], rng: &mut StdRng) -> GeneratedExpr {
    let matching = bindings
        .iter()
        .filter(|binding| binding.ty == ty)
        .collect::<Vec<_>>();
    if !matching.is_empty() && rng.random_bool(0.6) {
        let selected = matching[rng.random_range(0..matching.len())];
        return GeneratedExpr::variable(selected.name.clone(), ty);
    }
    match ty {
        GeneratedType::I64 => GeneratedExpr::I64(rng.random_range(-50..=50)),
        GeneratedType::Bool => GeneratedExpr::Bool(rng.random_bool(0.5)),
        GeneratedType::String => GeneratedExpr::Text(word(rng).to_string()),
        GeneratedType::VecI64 => {
            let count = rng.random_range(0..=5);
            GeneratedExpr::VecLiteral(
                (0..count)
                    .map(|_| GeneratedExpr::I64(rng.random_range(-20..=20)))
                    .collect(),
            )
        }
        GeneratedType::OptionI64 => {
            if rng.random_bool(0.7) {
                GeneratedExpr::Some(Box::new(GeneratedExpr::I64(rng.random_range(-30..=30))))
            } else {
                GeneratedExpr::None
            }
        }
    }
}

fn vec_literal(
    depth: usize,
    bindings: &[TypedBinding],
    rng: &mut StdRng,
    next_name: &mut usize,
) -> GeneratedExpr {
    let count = rng.random_range(0..=5);
    GeneratedExpr::VecLiteral(
        (0..count)
            .map(|_| expression(GeneratedType::I64, depth, bindings, rng, next_name))
            .collect(),
    )
}

fn fresh_name(prefix: &str, next_name: &mut usize) -> String {
    let name = format!("{prefix}_{}", *next_name);
    *next_name += 1;
    name
}

fn word(rng: &mut StdRng) -> &'static str {
    const WORDS: &[&str] = &[
        "",
        "a",
        "rust",
        "script",
        " line ",
        "λ",
        "two words",
        "line\nbreak",
    ];
    WORDS[rng.random_range(0..WORDS.len())]
}
