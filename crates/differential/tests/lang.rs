//! Guards for the type directed generator.
//!
//! The one that matters most is `generated_programs_compile`. A generator that
//! emits Rust the compiler rejects reports `RustcRejected` as a hard failure,
//! so a whole campaign turns into noise about the harness instead of findings
//! about the interpreter. Two real regressions were caught this way already,
//! both from a constant the compiler could fold into a lint error: an
//! unlaundered integer literal, then an unlaundered float literal cast to an
//! integer.

use std::collections::BTreeSet;

use rand::SeedableRng;
use rand::rngs::StdRng;
use rustscript_differential::lang::catalog::{METHODS, solve};
use rustscript_differential::lang::ty::{INT_WIDTHS, SCALAR_TYPES, Ty};
use rustscript_differential::lang::{Block, generate_block};
use rustscript_differential::runner::{Classification, Runner};
use rustscript_differential::workspace_root;

fn block_for(seed: u64) -> Block {
    let mut rng = StdRng::seed_from_u64(seed);
    generate_block(&mut rng)
}

/// A program made only of one generated block, so a compile failure points at
/// this generator and not at one of the older case lists.
fn program_for(seed: u64) -> String {
    let block = block_for(seed);
    let mut source = String::new();
    for helper in block.helpers() {
        source.push_str(helper.definition());
    }
    source.push_str("fn main() {\n");
    source.push_str(&block.render());
    source.push_str("}\n");
    source
}

#[test]
fn generation_is_deterministic() {
    for seed in [1, 7, 99, 4242] {
        assert_eq!(block_for(seed), block_for(seed));
        assert_eq!(program_for(seed), program_for(seed));
    }
}

/// Every generated program must be valid Rust. This is the contract that keeps
/// a campaign's findings about the interpreter rather than about the harness.
#[test]
fn generated_programs_compile() {
    let root = workspace_root();
    let runner = Runner::build(&root, 20_000).expect("build interpreter");
    let mut rejected = Vec::new();
    for seed in 0..60u64 {
        let source = program_for(seed);
        let result = runner.run_source(&source).expect("run generated program");
        if result.classification == Classification::RustcRejected {
            rejected.push((seed, result.compiler.stderr.clone(), source));
        }
    }
    assert!(
        rejected.is_empty(),
        "{} of 60 generated programs did not compile. First one, seed {}:\n{}\n\n{}",
        rejected.len(),
        rejected[0].0,
        rejected[0].1,
        rejected[0].2,
    );
}

/// The whole point of this generator is the type universe the old one could
/// not name. If a width stops appearing, the dimension is invisible again and
/// bugs living there become unfindable.
#[test]
fn generation_covers_the_type_universe() {
    let mut features = BTreeSet::new();
    for seed in 0..400u64 {
        block_for(seed).features(&mut features);
    }
    let expected = [
        "lang-ty-u8",
        "lang-ty-u16",
        "lang-ty-u32",
        "lang-ty-u64",
        "lang-ty-usize",
        "lang-ty-i8",
        "lang-ty-i16",
        "lang-ty-i32",
        "lang-ty-i64",
        "lang-ty-f32",
        "lang-ty-f64",
        "lang-ty-bool",
        "lang-ty-char",
        "lang-ty-string",
        "lang-ty-vec",
        "lang-ty-option",
        "lang-call",
        "lang-cast",
        "lang-if",
        "lang-for",
        "lang-op-add",
        "lang-op-shl",
        "lang-op-compare",
    ];
    let missing: Vec<&str> = expected
        .iter()
        .copied()
        .filter(|name| !features.contains(name))
        .collect();
    assert!(missing.is_empty(), "never generated: {missing:?}");
}

/// A method that no wanted type can ever solve against is dead weight in the
/// catalog: it is written down but the generator can never place a call to it.
#[test]
fn every_catalog_method_is_reachable() {
    let mut wanted: Vec<Ty> = SCALAR_TYPES.to_vec();
    for width in INT_WIDTHS {
        wanted.push(Ty::vec_of(Ty::Int(*width)));
        wanted.push(Ty::opt_of(Ty::Int(*width)));
    }
    for scalar in SCALAR_TYPES {
        wanted.push(Ty::vec_of(scalar.clone()));
        wanted.push(Ty::opt_of(scalar.clone()));
    }
    let unreachable: Vec<&str> = METHODS
        .iter()
        .filter(|method| !wanted.iter().any(|ty| solve(method, ty).is_some()))
        .map(|method| method.name)
        .collect();
    assert!(
        unreachable.is_empty(),
        "catalog methods no wanted type can reach: {unreachable:?}"
    );
}

/// Catalog keys are looked up by name when rendering, so a duplicate would
/// silently make one row render as another.
#[test]
fn catalog_names_are_unique() {
    let mut seen = BTreeSet::new();
    for method in METHODS {
        assert!(
            seen.insert(method.name),
            "duplicate catalog name `{}`",
            method.name
        );
    }
}
