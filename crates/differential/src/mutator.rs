use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

use crate::generator::generate_base;
use crate::model::{MutationOperation, MutationOrigin, Program};

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

pub fn mutate(parent: &Program, parent_seed: u64, donor_seed: u64, output_seed: u64) -> Program {
    let donor = generate_base(donor_seed);
    let mut rng = StdRng::seed_from_u64(output_seed ^ MUTATION_SALT);
    let mut operations = [
        MutationOperation::Adjustment,
        MutationOperation::Statements,
        MutationOperation::RichCases,
        MutationOperation::ClosureCases,
        MutationOperation::StructuralCases,
        MutationOperation::SemanticCases,
        MutationOperation::CaseOrder,
    ];
    for index in 0..operations.len() {
        let other = rng.random_range(index..operations.len());
        operations.swap(index, other);
    }

    let operation_count = rng.random_range(2..=4);
    let selected = operations[..operation_count].to_vec();
    let mut program = parent.clone();
    for operation in &selected {
        apply_operation(&mut program, &donor, *operation);
    }
    program.seed = output_seed;
    program.mutation = Some(MutationOrigin {
        parent_seed,
        donor_seed,
        operations: selected,
    });
    program
}

fn apply_operation(program: &mut Program, donor: &Program, operation: MutationOperation) {
    match operation {
        MutationOperation::Adjustment => program.adjustment = donor.adjustment,
        MutationOperation::Statements => program.statements.clone_from(&donor.statements),
        MutationOperation::RichCases => program.rich_cases.clone_from(&donor.rich_cases),
        MutationOperation::ClosureCases => program.closure_cases.clone_from(&donor.closure_cases),
        MutationOperation::StructuralCases => {
            program.structural_cases.clone_from(&donor.structural_cases);
        }
        MutationOperation::SemanticCases => {
            program.semantic_cases.clone_from(&donor.semantic_cases);
        }
        MutationOperation::CaseOrder => {
            program.rich_cases.reverse();
            program.closure_cases.reverse();
            program.structural_cases.reverse();
            program.semantic_cases.reverse();
        }
    }
}
