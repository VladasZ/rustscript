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
    generate_block(&mut rng, 0)
}

/// A program made only of one generated block, so a compile failure points at
/// this generator and not at one of the older case lists.
fn program_for(seed: u64) -> String {
    let block = block_for(seed);
    let mut source = String::new();
    let mut features = BTreeSet::new();
    block.features(&mut features);
    let uses_map = features.contains("lang-ty-map");
    let uses_set = features.contains("lang-ty-set");
    match (uses_map, uses_set) {
        (true, true) => source.push_str("use std::collections::{HashMap, HashSet};\n\n"),
        (true, false) => source.push_str("use std::collections::HashMap;\n\n"),
        (false, true) => source.push_str("use std::collections::HashSet;\n\n"),
        (false, false) => {}
    }
    for helper in block.helpers() {
        source.push_str(helper.definition());
    }
    source.push_str(&block.render_fns());
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
        "lang-ty-map",
        "lang-ty-set",
        "lang-pipe",
        "lang-pipe-collect-fish",
        "lang-pipe-collect-bare",
        "lang-fn-def",
        "lang-for-accum",
        "lang-mut-map-insert",
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

/// A closure that can panic must not run on an unordered stretch. Which item
/// panics first is the arrival order, and real Rust randomizes that order for
/// maps and sets, so the same program can print two different panic messages.
/// A nightly campaign hit exactly this: `into_values()` mapped through a
/// fallible body, sorted only afterwards.
#[test]
fn fallible_closure_needs_defined_order() {
    use rustscript_differential::lang::expr::{BinOp, Expr};
    use rustscript_differential::lang::pipe::{Access, Bind, Pipe, Site, Source, Stage, Term};
    use rustscript_differential::numeric::IntWidth;

    let int = |value: i128| Expr::IntLit {
        width: IntWidth::I64,
        value,
        opaque: true,
    };
    let fallible_map = Stage::Map {
        bind: Bind::One("diff_x_0".to_string()),
        body: Expr::Bin {
            op: BinOp::Add,
            left: Box::new(int(1)),
            right: Box::new(int(2)),
            ty: Ty::I64,
        },
    };
    let pipe_with = |stages: Vec<Stage>| Pipe {
        source: Source::Coll {
            expr: Expr::SetLit {
                elem: Ty::I64,
                items: Vec::new(),
            },
            access: Access::SetInto,
        },
        stages,
        term: Term::Collect {
            target: Ty::vec_of(Ty::I64),
            site: Site::Turbofish,
        },
    };

    // Sorting only after the fallible map is the nightly's failing shape.
    assert!(!pipe_with(vec![fallible_map.clone(), Stage::Sorted]).is_deterministic());
    // Sorting first defines the order the body runs in.
    assert!(pipe_with(vec![Stage::Sorted, fallible_map]).is_deterministic());
}
