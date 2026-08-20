//! Seeded program generation. Every fourth seed is a structured mutation of
//! its predecessor, see `mutator`.

use rand::RngExt;
use rand::SeedableRng;
use rand::rngs::StdRng;

use crate::lang::generate_block;
use crate::model::Program;

pub fn generate(seed: u64) -> Program {
    crate::mutator::generate_or_mutate(seed)
}

pub(crate) fn generate_base(seed: u64) -> Program {
    let mut rng = StdRng::seed_from_u64(seed);
    let count = rng.random_range(1..=2);
    let blocks = (0..count)
        .map(|tag| generate_block(&mut rng, tag))
        .collect();
    Program {
        seed,
        blocks,
        mutation: None,
    }
}
