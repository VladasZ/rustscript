use rand::RngExt;
use rand::rngs::StdRng;

use crate::typed::{GeneratedExpr, GeneratedType, IntCast};

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
        GeneratedType::F64 => f64_expression(depth, bindings, rng, next_name),
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
    match rng.random_range(0..20) {
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
        // Casts are the highest-yield divergence, so they carry extra weight.
        10..=12 => GeneratedExpr::Cast(
            boxed(GeneratedType::I64, child, bindings, rng, next_name),
            random_int_cast(rng),
        ),
        13 => raw_binary(GeneratedExpr::RawAdd, child, bindings, rng, next_name),
        14 => raw_binary(GeneratedExpr::RawMul, child, bindings, rng, next_name),
        15 => raw_binary(GeneratedExpr::RawSub, child, bindings, rng, next_name),
        16 => raw_division(rng, bindings),
        17 => GeneratedExpr::Index {
            values: boxed(GeneratedType::VecI64, child, bindings, rng, next_name),
            index: rng.random_range(0..=6),
        },
        18 => GeneratedExpr::Unwrap(boxed(
            GeneratedType::OptionI64,
            child,
            bindings,
            rng,
            next_name,
        )),
        // The saturating and NaN-to-zero semantics of a float-to-int cast are
        // easy for an interpreter to get wrong, so the cast gets a wild float
        // operand often.
        _ => GeneratedExpr::F64ToI64(boxed(GeneratedType::F64, child, bindings, rng, next_name)),
    }
}

fn f64_expression(
    depth: usize,
    bindings: &[TypedBinding],
    rng: &mut StdRng,
    next_name: &mut usize,
) -> GeneratedExpr {
    let child = depth - 1;
    match rng.random_range(0..9) {
        0 => leaf(GeneratedType::F64, bindings, rng),
        1 => GeneratedExpr::FAdd(
            boxed(GeneratedType::F64, child, bindings, rng, next_name),
            boxed(GeneratedType::F64, child, bindings, rng, next_name),
        ),
        2 => GeneratedExpr::FSub(
            boxed(GeneratedType::F64, child, bindings, rng, next_name),
            boxed(GeneratedType::F64, child, bindings, rng, next_name),
        ),
        3 => GeneratedExpr::FMul(
            boxed(GeneratedType::F64, child, bindings, rng, next_name),
            boxed(GeneratedType::F64, child, bindings, rng, next_name),
        ),
        4 => GeneratedExpr::FDiv(
            boxed(GeneratedType::F64, child, bindings, rng, next_name),
            boxed(GeneratedType::F64, child, bindings, rng, next_name),
        ),
        5 => if_expression(GeneratedType::F64, child, bindings, rng, next_name),
        6 => GeneratedExpr::I64ToF64(boxed(GeneratedType::I64, child, bindings, rng, next_name)),
        7 => match_option(GeneratedType::F64, child, bindings, rng, next_name),
        _ => closure_call(GeneratedType::F64, child, bindings, rng, next_name),
    }
}

/// A plain `+ - *` node. The left operand is forced to be an i64 variable so
/// the two operands are never both constant. That keeps the compiler's
/// const-overflow lint from rejecting the program while the overflow still
/// happens at runtime, which is exactly the divergence being hunted. With no
/// i64 variable in scope there is nothing safe to build on, so a leaf is used.
fn raw_binary(
    construct: fn(Box<GeneratedExpr>, Box<GeneratedExpr>) -> GeneratedExpr,
    depth: usize,
    bindings: &[TypedBinding],
    rng: &mut StdRng,
    next_name: &mut usize,
) -> GeneratedExpr {
    match pick_i64_var(bindings, rng) {
        Some(left) => construct(
            Box::new(left),
            boxed(GeneratedType::I64, depth, bindings, rng, next_name),
        ),
        None => leaf(GeneratedType::I64, bindings, rng),
    }
}

/// A plain `/ %` node. Both operands are variables so neither the divisor nor
/// the whole expression is a constant, which the const-panic lint would reject
/// on a literal zero divisor. A divide by zero or `i64::MIN / -1` then happens
/// only at runtime, where the interpreter and the compiler can disagree.
fn raw_division(rng: &mut StdRng, bindings: &[TypedBinding]) -> GeneratedExpr {
    match (pick_i64_var(bindings, rng), pick_i64_var(bindings, rng)) {
        (Some(left), Some(right)) => {
            let construct = if rng.random_bool(0.5) {
                GeneratedExpr::RawDiv
            } else {
                GeneratedExpr::RawRem
            };
            construct(Box::new(left), Box::new(right))
        }
        _ => leaf(GeneratedType::I64, bindings, rng),
    }
}

fn pick_i64_var(bindings: &[TypedBinding], rng: &mut StdRng) -> Option<GeneratedExpr> {
    let matching = bindings
        .iter()
        .filter(|binding| binding.ty == GeneratedType::I64)
        .collect::<Vec<_>>();
    let selected = matching.get(rng.random_range(0..matching.len().max(1)))?;
    Some(GeneratedExpr::variable(
        selected.name.clone(),
        GeneratedType::I64,
    ))
}

fn random_int_cast(rng: &mut StdRng) -> IntCast {
    match rng.random_range(0..8) {
        0 => IntCast::U8,
        1 => IntCast::U16,
        2 => IntCast::U32,
        3 => IntCast::U64,
        4 => IntCast::I8,
        5 => IntCast::I16,
        6 => IntCast::I32,
        _ => IntCast::USize,
    }
}

/// Boundary and extreme integers, so casts truncate and plain arithmetic
/// overflows instead of staying in the safe small range.
fn wild_i64(rng: &mut StdRng) -> i64 {
    const WILD: &[i64] = &[
        i64::MAX,
        i64::MIN,
        i64::MAX / 2,
        i64::MIN / 2,
        1 << 40,
        -(1 << 40),
        -1,
        255,
        256,
        65_535,
        65_536,
        2_147_483_647,
        -2_147_483_648,
        4_294_967_295,
    ];
    WILD[rng.random_range(0..WILD.len())]
}

/// Float literal tokens mixing tame values with the classic traps: negative
/// zero, values that round under bankers rounding, non-representable integers
/// past 2^53, extremes, subnormals, and the special constants.
fn wild_f64(rng: &mut StdRng) -> &'static str {
    const WILD: &[&str] = &[
        "0.0",
        "-0.0",
        "1.0",
        "-1.0",
        "0.5",
        "0.1",
        "1.5",
        "2.5",
        "-2.5",
        "10.25",
        "3.141592653589793",
        "0.30000000000000004",
        "1e300",
        "-1e300",
        "1e-300",
        "9007199254740993.0",
        "1.7976931348623157e308",
        "5e-324",
        "f64::NAN",
        "f64::INFINITY",
        "f64::NEG_INFINITY",
        "f64::EPSILON",
        "f64::MIN_POSITIVE",
    ];
    WILD[rng.random_range(0..WILD.len())]
}

fn bool_expression(
    depth: usize,
    bindings: &[TypedBinding],
    rng: &mut StdRng,
    next_name: &mut usize,
) -> GeneratedExpr {
    let child = depth - 1;
    match rng.random_range(0..12) {
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
        9 => closure_call(GeneratedType::Bool, child, bindings, rng, next_name),
        // NaN makes every ordered comparison false and equality asymmetric,
        // a classic interpreter trap.
        10 => GeneratedExpr::FLess(
            boxed(GeneratedType::F64, child, bindings, rng, next_name),
            boxed(GeneratedType::F64, child, bindings, rng, next_name),
        ),
        _ => GeneratedExpr::FEq(
            boxed(GeneratedType::F64, child, bindings, rng, next_name),
            boxed(GeneratedType::F64, child, bindings, rng, next_name),
        ),
    }
}

fn string_expression(
    depth: usize,
    bindings: &[TypedBinding],
    rng: &mut StdRng,
    next_name: &mut usize,
) -> GeneratedExpr {
    let child = depth - 1;
    match rng.random_range(0..13) {
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
        9 => GeneratedExpr::Concat(
            Box::new(GeneratedExpr::Text(word(rng).to_string())),
            Box::new(GeneratedExpr::FormatI64(boxed(
                GeneratedType::I64,
                child,
                bindings,
                rng,
                next_name,
            ))),
        ),
        10 => GeneratedExpr::FormatF64(boxed(GeneratedType::F64, child, bindings, rng, next_name)),
        11 => GeneratedExpr::DebugF64(boxed(GeneratedType::F64, child, bindings, rng, next_name)),
        _ => format_spec(child, bindings, rng, next_name),
    }
}

/// Format specs the standard library documents and the README claims: width,
/// fill, alignment, sign, zero padding, precision, radix, and exponent forms.
const I64_SPECS: &[&str] = &[
    ":>8", ":<8", ":^8", ":+", ":08", ":5", ":+09", ":x", ":X", ":#x", ":#X", ":o", ":#o", ":b",
    ":#b", ":#010x", ":e", ":E", ":*>12", ":*^12", ":->6",
];
const F64_SPECS: &[&str] = &[
    ":.3", ":.0", ":+.2", ":10.3", ":e", ":E", ":08.2", ":>12.4", ":+", ":.7",
];
const STRING_SPECS: &[&str] = &[":>10", ":<10", ":^10", ":.3", ":10.4", ":-^12", ":.0"];

fn format_spec(
    depth: usize,
    bindings: &[TypedBinding],
    rng: &mut StdRng,
    next_name: &mut usize,
) -> GeneratedExpr {
    let (ty, specs) = match rng.random_range(0..3) {
        0 => (GeneratedType::I64, I64_SPECS),
        1 => (GeneratedType::F64, F64_SPECS),
        _ => (GeneratedType::String, STRING_SPECS),
    };
    GeneratedExpr::FormatSpec {
        spec: specs[rng.random_range(0..specs.len())].to_string(),
        value: boxed(ty, depth, bindings, rng, next_name),
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
        GeneratedType::I64 => {
            if rng.random_bool(0.35) {
                GeneratedExpr::I64(wild_i64(rng))
            } else {
                GeneratedExpr::I64(rng.random_range(-50..=50))
            }
        }
        GeneratedType::F64 => GeneratedExpr::F64(wild_f64(rng).to_string()),
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
