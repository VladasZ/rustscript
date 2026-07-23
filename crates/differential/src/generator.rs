use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

use crate::closure_case::ClosureCase;
use crate::model::{Expr, Program, Stmt, Ty};
use crate::rich::{RichCase, StateVariant};
use crate::semantic_gen::generate_semantic_cases;
use crate::structural_gen::generate_structural_cases;

#[derive(Clone)]
struct Binding {
    name: String,
    ty: Ty,
}

pub fn generate(seed: u64) -> Program {
    crate::mutator::generate_or_mutate(seed)
}

pub(crate) fn generate_base(seed: u64) -> Program {
    let mut rng = StdRng::seed_from_u64(seed);
    let adjustment = rng.random_range(0..=8);
    let count = rng.random_range(4..=7);
    let mut bindings = Vec::new();
    let mut statements = Vec::new();

    for index in 0..count {
        let ty = match index {
            0 => Ty::I64,
            1 => Ty::Bool,
            2 => Ty::String,
            _ => random_ty(&mut rng),
        };
        let name = format!("value_{index}");
        let expr = if index == 0 {
            Expr::Adjust {
                value: Box::new(Expr::I64(rng.random_range(-20..=20))),
                flag: Box::new(Expr::Bool(rng.random_bool(0.5))),
            }
        } else {
            generate_expr(ty, 2, &bindings, &mut rng)
        };
        statements.push(Stmt::Let {
            name: name.clone(),
            ty,
            expr,
        });
        bindings.push(Binding { name, ty });
    }

    let mutations = rng.random_range(2..=5);
    for _ in 0..mutations {
        let binding = bindings[rng.random_range(0..bindings.len())].clone();
        match rng.random_range(0..3) {
            0 => statements.push(Stmt::Assign {
                name: binding.name,
                expr: generate_expr(binding.ty, 2, &bindings, &mut rng),
            }),
            1 => statements.push(Stmt::IfAssign {
                name: binding.name,
                condition: generate_expr(Ty::Bool, 2, &bindings, &mut rng),
                then_expr: generate_expr(binding.ty, 2, &bindings, &mut rng),
                else_expr: generate_expr(binding.ty, 2, &bindings, &mut rng),
            }),
            _ if binding.ty == Ty::I64 => statements.push(Stmt::ForAdd {
                name: binding.name,
                iterations: rng.random_range(0..=4),
                delta: rng.random_range(-4..=4),
            }),
            _ => statements.push(Stmt::Assign {
                name: binding.name,
                expr: generate_expr(binding.ty, 2, &bindings, &mut rng),
            }),
        }
    }

    Program {
        seed,
        adjustment,
        statements,
        rich_cases: generate_rich_cases(&mut rng),
        closure_cases: generate_closure_cases(&mut rng),
        structural_cases: generate_structural_cases(&mut rng),
        semantic_cases: generate_semantic_cases(&mut rng),
        mutation: None,
    }
}

fn generate_closure_cases(rng: &mut StdRng) -> Vec<ClosureCase> {
    let mut kinds = [0, 1, 2, 3, 4, 5];
    let count = rng.random_range(0..=2);
    for index in 0..count {
        let swap_with = rng.random_range(index..kinds.len());
        kinds.swap(index, swap_with);
    }
    kinds[..count]
        .iter()
        .map(|kind| generate_closure_case(*kind, rng))
        .collect()
}

fn generate_closure_case(kind: usize, rng: &mut StdRng) -> ClosureCase {
    match kind {
        0 => ClosureCase::Nested {
            input: rng.random_range(-10..=10),
            outer_bias: rng.random_range(-10..=10),
            inner_bias: rng.random_range(-10..=10),
            arguments: [rng.random_range(-5..=5), rng.random_range(-5..=5)],
        },
        1 => ClosureCase::MutableCapture {
            values: random_values(rng, 7),
            initial: rng.random_range(-20..=20),
        },
        2 => ClosureCase::MoveString {
            prefix: choose(rng, &["", "item", "rust", "λ", " line "]).to_string(),
            suffixes: random_strings(rng, 5),
        },
        3 => ClosureCase::CapturedCall {
            values: random_values(rng, 7),
            bias: rng.random_range(-10..=10),
            threshold: rng.random_range(-10..=10),
        },
        4 => ClosureCase::TuplePattern {
            pairs: random_pairs(rng, 6),
            multiplier: rng.random_range(-3..=3),
        },
        _ => ClosureCase::GenericApply {
            input: rng.random_range(-20..=20),
            delta: rng.random_range(-5..=5),
            times: rng.random_range(0..=5),
        },
    }
}

fn generate_rich_cases(rng: &mut StdRng) -> Vec<RichCase> {
    let mut cases = vec![
        RichCase::OptionClosure {
            input: random_option_i64(rng),
            threshold: rng.random_range(-10..=20),
            multiplier: rng.random_range(-3..=4),
            offset: rng.random_range(-10..=10),
            fallback: random_option_i64(rng),
        },
        RichCase::VectorPipeline {
            values: random_values(rng, 7),
            multiplier: rng.random_range(-3..=4),
            offset: rng.random_range(-10..=10),
            minimum: rng.random_range(-20..=20),
            extra: rng.random_range(-20..=20),
            lookup: rng.random_range(0..=8),
            reverse: rng.random_bool(0.5),
        },
        RichCase::PathString {
            raw: choose(
                rng,
                &[
                    " src/lib.rs ",
                    "assets/icon_dark.png",
                    "./data/report_final.txt",
                    r"one\two\file_name.rs",
                    "README",
                    "λ/δοκιμή.txt",
                ],
            )
            .to_string(),
            child: if rng.random_bool(0.55) {
                Some(
                    choose(
                        rng,
                        &[
                            "nested/item_one.log",
                            "child-file.txt",
                            "more/data.json",
                            "λ.txt",
                        ],
                    )
                    .to_string(),
                )
            } else {
                None
            },
            extension: choose(rng, &["rs", "txt", "bak", "", "data"]).to_string(),
            separator: *choose(rng, &['_', '-', '.']),
            needle: choose(rng, &["", "a", "i", "rust", "λ"]).to_string(),
            replacement: choose(rng, &["x", "_", "R", "", "λ"]).to_string(),
            uppercase: rng.random_bool(0.5),
        },
        RichCase::EnumMatch {
            variant: match rng.random_range(0..3) {
                0 => StateVariant::Idle,
                1 => StateVariant::Named,
                _ => StateVariant::Located,
            },
            label: choose(rng, &["", "ready-state", "rust item", "λ", "missing"]).to_string(),
            path: choose(
                rng,
                &["src/main.rs", "assets/data.bin", "README", "λ/item.txt"],
            )
            .to_string(),
            values: random_values(rng, 5),
            needle: choose(rng, &["", "rust", "ready", "λ", "missing"]).to_string(),
            bias: rng.random_range(-10..=10),
        },
    ];
    let count = rng.random_range(0..=2);
    for index in 0..count {
        let swap_with = rng.random_range(index..cases.len());
        cases.swap(index, swap_with);
    }
    cases.truncate(count);
    cases
}

fn random_option_i64(rng: &mut StdRng) -> Option<i64> {
    if rng.random_bool(0.7) {
        Some(rng.random_range(-20..=30))
    } else {
        None
    }
}

fn random_values(rng: &mut StdRng, maximum: usize) -> Vec<i64> {
    let count = rng.random_range(0..=maximum);
    (0..count).map(|_| rng.random_range(-20..=20)).collect()
}

fn random_strings(rng: &mut StdRng, maximum: usize) -> Vec<String> {
    let count = rng.random_range(0..=maximum);
    (0..count)
        .map(|_| choose(rng, &["", " a ", "rust", "script ", "λ"]).to_string())
        .collect()
}

fn random_pairs(rng: &mut StdRng, maximum: usize) -> Vec<(i64, i64)> {
    let count = rng.random_range(0..=maximum);
    (0..count)
        .map(|_| (rng.random_range(-10..=10), rng.random_range(-10..=10)))
        .collect()
}

fn choose<'a, T>(rng: &mut StdRng, values: &'a [T]) -> &'a T {
    &values[rng.random_range(0..values.len())]
}

fn random_ty(rng: &mut StdRng) -> Ty {
    match rng.random_range(0..3) {
        0 => Ty::I64,
        1 => Ty::Bool,
        _ => Ty::String,
    }
}

fn generate_expr(ty: Ty, depth: usize, bindings: &[Binding], rng: &mut StdRng) -> Expr {
    if depth == 0 {
        return leaf(ty, bindings, rng);
    }
    match ty {
        Ty::I64 => match rng.random_range(0..6) {
            0 => leaf(ty, bindings, rng),
            1 => Expr::SaturatingAdd(
                Box::new(generate_expr(ty, depth - 1, bindings, rng)),
                Box::new(generate_expr(ty, depth - 1, bindings, rng)),
            ),
            2 => Expr::SaturatingSub(
                Box::new(generate_expr(ty, depth - 1, bindings, rng)),
                Box::new(generate_expr(ty, depth - 1, bindings, rng)),
            ),
            3 => Expr::SaturatingMul(
                Box::new(generate_expr(ty, depth - 1, bindings, rng)),
                Box::new(Expr::I64(rng.random_range(-4..=4))),
            ),
            4 => Expr::If {
                condition: Box::new(generate_expr(Ty::Bool, depth - 1, bindings, rng)),
                then_expr: Box::new(generate_expr(ty, depth - 1, bindings, rng)),
                else_expr: Box::new(generate_expr(ty, depth - 1, bindings, rng)),
                ty,
            },
            _ => Expr::Adjust {
                value: Box::new(generate_expr(ty, depth - 1, bindings, rng)),
                flag: Box::new(generate_expr(Ty::Bool, depth - 1, bindings, rng)),
            },
        },
        Ty::Bool => match rng.random_range(0..7) {
            0 => leaf(ty, bindings, rng),
            1 => Expr::Eq(
                Box::new(generate_expr(Ty::I64, depth - 1, bindings, rng)),
                Box::new(generate_expr(Ty::I64, depth - 1, bindings, rng)),
            ),
            2 => Expr::Less(
                Box::new(generate_expr(Ty::I64, depth - 1, bindings, rng)),
                Box::new(generate_expr(Ty::I64, depth - 1, bindings, rng)),
            ),
            3 => Expr::And(
                Box::new(generate_expr(ty, depth - 1, bindings, rng)),
                Box::new(generate_expr(ty, depth - 1, bindings, rng)),
            ),
            4 => Expr::Or(
                Box::new(generate_expr(ty, depth - 1, bindings, rng)),
                Box::new(generate_expr(ty, depth - 1, bindings, rng)),
            ),
            5 => Expr::Not(Box::new(generate_expr(ty, depth - 1, bindings, rng))),
            _ => Expr::If {
                condition: Box::new(generate_expr(ty, depth - 1, bindings, rng)),
                then_expr: Box::new(generate_expr(ty, depth - 1, bindings, rng)),
                else_expr: Box::new(generate_expr(ty, depth - 1, bindings, rng)),
                ty,
            },
        },
        Ty::String => match rng.random_range(0..5) {
            0 => leaf(ty, bindings, rng),
            1 => Expr::Concat(
                Box::new(generate_expr(ty, depth - 1, bindings, rng)),
                Box::new(generate_expr(ty, depth - 1, bindings, rng)),
            ),
            2 => Expr::Repeat(
                Box::new(generate_expr(ty, depth - 1, bindings, rng)),
                rng.random_range(0..=3),
            ),
            _ => Expr::If {
                condition: Box::new(generate_expr(Ty::Bool, depth - 1, bindings, rng)),
                then_expr: Box::new(generate_expr(ty, depth - 1, bindings, rng)),
                else_expr: Box::new(generate_expr(ty, depth - 1, bindings, rng)),
                ty,
            },
        },
    }
}

fn leaf(ty: Ty, bindings: &[Binding], rng: &mut StdRng) -> Expr {
    let matching: Vec<&Binding> = bindings.iter().filter(|binding| binding.ty == ty).collect();
    if !matching.is_empty() && rng.random_bool(0.55) {
        let binding = matching[rng.random_range(0..matching.len())];
        return Expr::Var {
            name: binding.name.clone(),
            ty,
        };
    }
    match ty {
        Ty::I64 => Expr::I64(rng.random_range(-50..=50)),
        Ty::Bool => Expr::Bool(rng.random_bool(0.5)),
        Ty::String => {
            const WORDS: &[&str] = &["", "a", "rust", "script", "λ", "line\nbreak"];
            Expr::Text(WORDS[rng.random_range(0..WORDS.len())].to_string())
        }
    }
}
