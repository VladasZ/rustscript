use rand::RngExt;
use rand::rngs::StdRng;

use crate::structural::{
    DataflowCase, EnumCase, FlowStatement, FunctionCase, FunctionParameter, GeneratedBinding,
    GeneratedEnumVariant, MutableClosureCase, MutableClosureKind, StructuralCase,
};
use crate::typed::{GeneratedExpr, GeneratedType};
use crate::typed_gen::{TypedBinding, expression};

pub fn generate_structural_cases(rng: &mut StdRng) -> Vec<StructuralCase> {
    let count = rng.random_range(3..=6);
    let mut cases = vec![StructuralCase::Dataflow(generate_dataflow(0, rng))];
    for id in 1..count {
        let case = match rng.random_range(0..4) {
            0 => StructuralCase::Dataflow(generate_dataflow(id, rng)),
            1 => StructuralCase::MutableClosure(generate_mutable_closure(id, rng)),
            2 => StructuralCase::Enum(Box::new(generate_enum(id, rng))),
            _ => StructuralCase::Function(generate_function(id, rng)),
        };
        cases.push(case);
    }
    cases
}

fn generate_dataflow(id: usize, rng: &mut StdRng) -> DataflowCase {
    let count = rng.random_range(5..=10);
    let mut types = vec![
        GeneratedType::I64,
        GeneratedType::Bool,
        GeneratedType::String,
        GeneratedType::VecI64,
        GeneratedType::OptionI64,
    ];
    while types.len() < count {
        types.push(random_type(rng));
    }
    shuffle(&mut types, rng);

    let mut next_name = id * 10_000;
    let mut environment = Vec::new();
    let mut bindings = Vec::new();
    for (index, ty) in types.into_iter().enumerate() {
        let name = format!("flow_{id}_value_{index}");
        let depth = rng.random_range(2..=4);
        let expr = expression(ty, depth, &environment, rng, &mut next_name);
        bindings.push(GeneratedBinding {
            name: name.clone(),
            ty,
            expr,
            mutable: false,
        });
        environment.push(TypedBinding { name, ty });
    }

    let statement_count = rng.random_range(2..=6);
    let mut statements = Vec::new();
    for statement_index in 0..statement_count {
        let target_index = rng.random_range(0..bindings.len());
        let target = bindings[target_index].name.clone();
        let ty = bindings[target_index].ty;
        bindings[target_index].mutable = true;
        let depth = rng.random_range(2..=4);
        let statement = match rng.random_range(0..3) {
            0 => FlowStatement::Assign {
                target,
                value: expression(ty, depth, &environment, rng, &mut next_name),
            },
            1 => FlowStatement::IfAssign {
                target,
                condition: expression(
                    GeneratedType::Bool,
                    depth,
                    &environment,
                    rng,
                    &mut next_name,
                ),
                then_value: expression(ty, depth, &environment, rng, &mut next_name),
                else_value: expression(ty, depth, &environment, rng, &mut next_name),
            },
            _ if ty == GeneratedType::I64 => {
                let index = format!("flow_{id}_index_{statement_index}");
                let mut loop_environment = environment.clone();
                loop_environment.push(TypedBinding {
                    name: index.clone(),
                    ty: GeneratedType::I64,
                });
                let generated = expression(ty, depth, &loop_environment, rng, &mut next_name);
                FlowStatement::LoopAssign {
                    target: target.clone(),
                    index,
                    iterations: rng.random_range(0..=5),
                    value: GeneratedExpr::Add(
                        Box::new(GeneratedExpr::variable(target, GeneratedType::I64)),
                        Box::new(generated),
                    ),
                }
            }
            _ => FlowStatement::Assign {
                target,
                value: expression(ty, depth, &environment, rng, &mut next_name),
            },
        };
        statements.push(statement);
    }

    DataflowCase {
        id,
        bindings,
        statements,
    }
}

fn generate_mutable_closure(id: usize, rng: &mut StdRng) -> MutableClosureCase {
    let state = format!("closure_state_{id}");
    let item = format!("closure_item_{id}");
    let scale = format!("closure_scale_{id}");
    let bias = format!("closure_bias_{id}");
    let environment = vec![
        TypedBinding {
            name: state.clone(),
            ty: GeneratedType::I64,
        },
        TypedBinding {
            name: item.clone(),
            ty: GeneratedType::I64,
        },
        TypedBinding {
            name: scale.clone(),
            ty: GeneratedType::I64,
        },
        TypedBinding {
            name: bias,
            ty: GeneratedType::I64,
        },
    ];
    let mut next_name = id * 10_000 + 5_000;
    let generated = expression(
        GeneratedType::I64,
        rng.random_range(2..=4),
        &environment,
        rng,
        &mut next_name,
    );
    let item_delta = GeneratedExpr::Multiply(
        Box::new(GeneratedExpr::variable(item, GeneratedType::I64)),
        Box::new(GeneratedExpr::variable(scale, GeneratedType::I64)),
    );
    let update = GeneratedExpr::Add(
        Box::new(GeneratedExpr::variable(state, GeneratedType::I64)),
        Box::new(GeneratedExpr::Add(
            Box::new(item_delta),
            Box::new(generated),
        )),
    );
    MutableClosureCase {
        id,
        kind: match rng.random_range(0..3) {
            0 => MutableClosureKind::BorrowedMap,
            1 => MutableClosureKind::BorrowedLoop,
            _ => MutableClosureKind::OwnedFactory,
        },
        initial: rng.random_range(-20..=20),
        scale: rng.random_range(-3..=3),
        bias: rng.random_range(-10..=10),
        values: random_i64_values(rng, 7),
        update,
    }
}

fn generate_enum(id: usize, rng: &mut StdRng) -> EnumCase {
    let mut next_name = id * 10_000 + 6_000;
    let empty = Vec::new();
    let number = environment(format!("enum_number_{id}"), GeneratedType::I64);
    let text = environment(format!("enum_text_{id}"), GeneratedType::String);
    let pair = vec![
        TypedBinding {
            name: format!("enum_left_{id}"),
            ty: GeneratedType::I64,
        },
        TypedBinding {
            name: format!("enum_right_{id}"),
            ty: GeneratedType::I64,
        },
    ];
    let values = environment(format!("enum_values_{id}"), GeneratedType::VecI64);
    let some = environment(format!("enum_some_{id}"), GeneratedType::I64);
    let depth = rng.random_range(2..=4);
    EnumCase {
        id,
        variant: match rng.random_range(0..7) {
            0 => GeneratedEnumVariant::Unit,
            1 => GeneratedEnumVariant::Number,
            2 => GeneratedEnumVariant::Text,
            3 => GeneratedEnumVariant::Pair,
            4 => GeneratedEnumVariant::Values,
            5 => GeneratedEnumVariant::MaybeSome,
            _ => GeneratedEnumVariant::MaybeNone,
        },
        number: rng.random_range(-30..=30),
        text: word(rng).to_string(),
        values: random_i64_values(rng, 6),
        guard_length: rng.random_range(0..=8),
        unit_arm: expression(GeneratedType::String, depth, &empty, rng, &mut next_name),
        number_arm: expression(GeneratedType::String, depth, &number, rng, &mut next_name),
        text_guard_arm: expression(GeneratedType::String, depth, &text, rng, &mut next_name),
        text_arm: expression(GeneratedType::String, depth, &text, rng, &mut next_name),
        pair_arm: expression(GeneratedType::String, depth, &pair, rng, &mut next_name),
        values_arm: expression(GeneratedType::String, depth, &values, rng, &mut next_name),
        some_arm: expression(GeneratedType::String, depth, &some, rng, &mut next_name),
        none_arm: expression(GeneratedType::String, depth, &empty, rng, &mut next_name),
    }
}

fn generate_function(id: usize, rng: &mut StdRng) -> FunctionCase {
    let parameter_count = rng.random_range(1..=4);
    let mut parameters = Vec::new();
    let mut environment = Vec::new();
    for index in 0..parameter_count {
        let ty = random_type(rng);
        let name = format!("function_{id}_argument_{index}");
        parameters.push(FunctionParameter {
            name: name.clone(),
            ty,
        });
        environment.push(TypedBinding { name, ty });
    }
    let return_type = random_type(rng);
    let mut next_name = id * 10_000 + 8_000;
    let body = expression(
        return_type,
        rng.random_range(3..=5),
        &environment,
        rng,
        &mut next_name,
    );
    let empty = Vec::new();
    let arguments = parameters
        .iter()
        .map(|parameter| {
            expression(
                parameter.ty,
                rng.random_range(1..=3),
                &empty,
                rng,
                &mut next_name,
            )
        })
        .collect();
    FunctionCase {
        id,
        parameters,
        return_type,
        body,
        arguments,
        calls: rng.random_range(1..=3),
    }
}

fn environment(name: String, ty: GeneratedType) -> Vec<TypedBinding> {
    vec![TypedBinding { name, ty }]
}

fn random_type(rng: &mut StdRng) -> GeneratedType {
    match rng.random_range(0..6) {
        0 => GeneratedType::I64,
        1 => GeneratedType::Bool,
        2 => GeneratedType::String,
        3 => GeneratedType::VecI64,
        4 => GeneratedType::F64,
        _ => GeneratedType::OptionI64,
    }
}

fn random_i64_values(rng: &mut StdRng, maximum: usize) -> Vec<i64> {
    let count = rng.random_range(0..=maximum);
    (0..count).map(|_| rng.random_range(-20..=20)).collect()
}

fn shuffle<T>(values: &mut [T], rng: &mut StdRng) {
    for index in 0..values.len() {
        let other = rng.random_range(index..values.len());
        values.swap(index, other);
    }
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
