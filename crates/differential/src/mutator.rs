use rand::RngExt;
use rand::SeedableRng;
use rand::rngs::StdRng;

use crate::generator::generate_base;
use crate::model::{MutationOperation, MutationOrigin, Program};
use crate::structural::{EnumCase, FlowStatement, StructuralCase};
use crate::typed::{GeneratedExpr, GeneratedType};
use crate::typed_gen::TypedBinding;

const MUTATION_SALT: u64 = 0x9e37_79b9_7f4a_7c15;

pub fn generate_or_mutate(seed: u64) -> Program {
    if seed != 0 && seed.is_multiple_of(4) {
        let parent_seed = seed.wrapping_sub(1);
        let parent = generate_base(parent_seed);
        mutate(&parent, parent_seed, seed, seed)
    } else {
        generate_base(seed)
    }
}

/// Structured mutation by subtree splicing: pick a node somewhere in the
/// parent's expression trees, pick a same-typed subtree from the donor, fix
/// the donor's free variables up for the target scope, and drop it in. This
/// creates nesting shapes the top-down generator never emits while keeping
/// the program compile-valid by construction.
pub fn mutate(parent: &Program, parent_seed: u64, donor_seed: u64, output_seed: u64) -> Program {
    let donor = generate_base(donor_seed);
    let mut rng = StdRng::seed_from_u64(output_seed ^ MUTATION_SALT);
    let operation_count = rng.random_range(2..=4);
    let mut program = parent.clone();
    let mut operations = Vec::with_capacity(operation_count);
    for _ in 0..operation_count {
        if rng.random_bool(0.85) && splice(&mut program, &donor, &mut rng) {
            operations.push(MutationOperation::Splice);
        } else {
            reverse_case_order(&mut program);
            operations.push(MutationOperation::CaseOrder);
        }
    }
    program.seed = output_seed;
    program.mutation = Some(MutationOrigin {
        parent_seed,
        donor_seed,
        operations,
    });
    program
}

fn reverse_case_order(program: &mut Program) {
    program.rich_cases.reverse();
    program.closure_cases.reverse();
    program.structural_cases.reverse();
    program.semantic_cases.reverse();
    program.method_cases.reverse();
}

/// One expression slot a splice may target, with the bindings its rendered
/// position can see.
struct Slot<'a> {
    expr: &'a mut GeneratedExpr,
    environment: Vec<TypedBinding>,
}

fn splice(program: &mut Program, donor: &Program, rng: &mut StdRng) -> bool {
    if program.structural_cases.is_empty() {
        return false;
    }
    let case_index = rng.random_range(0..program.structural_cases.len());
    let mut slots = case_slots(&mut program.structural_cases[case_index]);
    if slots.is_empty() {
        return false;
    }
    let slot_index = rng.random_range(0..slots.len());
    let slot = &mut slots[slot_index];
    let node_count = slot.expr.nodes().len();
    let node_index = rng.random_range(0..node_count);
    let Some(target) = slot.expr.nth_node_mut(node_index) else {
        return false;
    };
    let wanted = target.ty();

    let donor_nodes: Vec<&GeneratedExpr> = donor
        .structural_cases
        .iter()
        .flat_map(donor_expressions)
        .flat_map(GeneratedExpr::nodes)
        .filter(|node| node.ty() == wanted)
        .collect();
    if donor_nodes.is_empty() {
        return false;
    }
    let mut graft = donor_nodes[rng.random_range(0..donor_nodes.len())].clone();
    let mut bound = Vec::new();
    rebind(&mut graft, &slot.environment, &mut bound, rng);
    *target = graft;
    true
}

/// Rewire the graft's free variables to bindings the target scope actually
/// has. Names bound inside the graft itself, closure and match bindings,
/// travel with it and stay untouched.
fn rebind(
    expr: &mut GeneratedExpr,
    environment: &[TypedBinding],
    bound: &mut Vec<String>,
    rng: &mut StdRng,
) {
    match expr {
        GeneratedExpr::Variable { name, ty } => {
            if bound.iter().any(|b| b == name) {
                return;
            }
            let candidates: Vec<&TypedBinding> = environment
                .iter()
                .filter(|binding| binding.ty == *ty)
                .collect();
            if candidates.is_empty() {
                *expr = minimal_literal(*ty);
            } else {
                *name = candidates[rng.random_range(0..candidates.len())]
                    .name
                    .clone();
            }
        }
        GeneratedExpr::VecMap {
            values,
            binding,
            body,
        }
        | GeneratedExpr::VecFilter {
            values,
            binding,
            predicate: body,
        }
        | GeneratedExpr::OptionMap {
            option: values,
            binding,
            body,
        }
        | GeneratedExpr::OptionFilter {
            option: values,
            binding,
            predicate: body,
        } => {
            rebind(values, environment, bound, rng);
            bound.push(binding.clone());
            rebind(body, environment, bound, rng);
            bound.pop();
        }
        GeneratedExpr::MatchOption {
            option,
            binding,
            some,
            none,
            ..
        } => {
            rebind(option, environment, bound, rng);
            bound.push(binding.clone());
            rebind(some, environment, bound, rng);
            bound.pop();
            rebind(none, environment, bound, rng);
        }
        GeneratedExpr::ClosureCall {
            binding,
            input,
            body,
            ..
        } => {
            rebind(input, environment, bound, rng);
            bound.push(binding.clone());
            rebind(body, environment, bound, rng);
            bound.pop();
        }
        other => {
            for child in other.children_mut() {
                rebind(child, environment, bound, rng);
            }
        }
    }
}

fn minimal_literal(ty: GeneratedType) -> GeneratedExpr {
    match ty {
        GeneratedType::I64 => GeneratedExpr::I64(0),
        GeneratedType::F64 => GeneratedExpr::F64("0.0".to_string()),
        GeneratedType::Bool => GeneratedExpr::Bool(false),
        GeneratedType::String => GeneratedExpr::Text(String::new()),
        GeneratedType::VecI64 => GeneratedExpr::VecLiteral(Vec::new()),
        GeneratedType::OptionI64 => GeneratedExpr::None,
    }
}

fn binding(name: String, ty: GeneratedType) -> TypedBinding {
    TypedBinding { name, ty }
}

/// The expression slots of one structural case together with what each can
/// see, mirroring how the generators build their environments.
fn case_slots(case: &mut StructuralCase) -> Vec<Slot<'_>> {
    match case {
        StructuralCase::Dataflow(dataflow) => {
            let visible: Vec<TypedBinding> = dataflow
                .bindings
                .iter()
                .map(|b| binding(b.name.clone(), b.ty))
                .collect();
            let mut slots = Vec::new();
            for (index, flow_binding) in dataflow.bindings.iter_mut().enumerate() {
                slots.push(Slot {
                    expr: &mut flow_binding.expr,
                    environment: visible[..index].to_vec(),
                });
            }
            for statement in &mut dataflow.statements {
                match statement {
                    FlowStatement::Assign { value, .. } => slots.push(Slot {
                        expr: value,
                        environment: visible.clone(),
                    }),
                    FlowStatement::IfAssign {
                        condition,
                        then_value,
                        else_value,
                        ..
                    } => {
                        for expr in [condition, then_value, else_value] {
                            slots.push(Slot {
                                expr,
                                environment: visible.clone(),
                            });
                        }
                    }
                    FlowStatement::LoopAssign { index, value, .. } => {
                        let mut environment = visible.clone();
                        environment.push(binding(index.clone(), GeneratedType::I64));
                        slots.push(Slot {
                            expr: value,
                            environment,
                        });
                    }
                }
            }
            slots
        }
        StructuralCase::MutableClosure(closure) => {
            let id = closure.id;
            let environment = vec![
                binding(format!("closure_state_{id}"), GeneratedType::I64),
                binding(format!("closure_item_{id}"), GeneratedType::I64),
                binding(format!("closure_scale_{id}"), GeneratedType::I64),
                binding(format!("closure_bias_{id}"), GeneratedType::I64),
            ];
            vec![Slot {
                expr: &mut closure.update,
                environment,
            }]
        }
        StructuralCase::Enum(enum_case) => enum_slots(enum_case),
        StructuralCase::Function(function) => {
            let environment: Vec<TypedBinding> = function
                .parameters
                .iter()
                .map(|parameter| binding(parameter.name.clone(), parameter.ty))
                .collect();
            let mut slots = vec![Slot {
                expr: &mut function.body,
                environment,
            }];
            for argument in &mut function.arguments {
                slots.push(Slot {
                    expr: argument,
                    environment: Vec::new(),
                });
            }
            slots
        }
    }
}

fn enum_slots(enum_case: &mut EnumCase) -> Vec<Slot<'_>> {
    let id = enum_case.id;
    let number = vec![binding(format!("enum_number_{id}"), GeneratedType::I64)];
    let text = vec![binding(format!("enum_text_{id}"), GeneratedType::String)];
    let pair = vec![
        binding(format!("enum_left_{id}"), GeneratedType::I64),
        binding(format!("enum_right_{id}"), GeneratedType::I64),
    ];
    let values = vec![binding(format!("enum_values_{id}"), GeneratedType::VecI64)];
    let some = vec![binding(format!("enum_some_{id}"), GeneratedType::I64)];
    vec![
        Slot {
            expr: &mut enum_case.unit_arm,
            environment: Vec::new(),
        },
        Slot {
            expr: &mut enum_case.number_arm,
            environment: number,
        },
        Slot {
            expr: &mut enum_case.text_guard_arm,
            environment: text.clone(),
        },
        Slot {
            expr: &mut enum_case.text_arm,
            environment: text,
        },
        Slot {
            expr: &mut enum_case.pair_arm,
            environment: pair,
        },
        Slot {
            expr: &mut enum_case.values_arm,
            environment: values,
        },
        Slot {
            expr: &mut enum_case.some_arm,
            environment: some,
        },
        Slot {
            expr: &mut enum_case.none_arm,
            environment: Vec::new(),
        },
    ]
}

/// The donor side reads the same slots immutably.
fn donor_expressions(case: &StructuralCase) -> Vec<&GeneratedExpr> {
    match case {
        StructuralCase::Dataflow(dataflow) => {
            let mut exprs: Vec<&GeneratedExpr> =
                dataflow.bindings.iter().map(|b| &b.expr).collect();
            for statement in &dataflow.statements {
                match statement {
                    FlowStatement::Assign { value, .. }
                    | FlowStatement::LoopAssign { value, .. } => exprs.push(value),
                    FlowStatement::IfAssign {
                        condition,
                        then_value,
                        else_value,
                        ..
                    } => exprs.extend([condition, then_value, else_value]),
                }
            }
            exprs
        }
        StructuralCase::MutableClosure(closure) => vec![&closure.update],
        StructuralCase::Enum(enum_case) => vec![
            &enum_case.unit_arm,
            &enum_case.number_arm,
            &enum_case.text_guard_arm,
            &enum_case.text_arm,
            &enum_case.pair_arm,
            &enum_case.values_arm,
            &enum_case.some_arm,
            &enum_case.none_arm,
        ],
        StructuralCase::Function(function) => {
            let mut exprs = vec![&function.body];
            exprs.extend(function.arguments.iter());
            exprs
        }
    }
}
