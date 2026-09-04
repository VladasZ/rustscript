//! Guards for the type directed generator. The one that matters most is `generated_programs_compile`,
//! a rejected program turns a campaign into noise about the harness.

use std::collections::BTreeSet;

use rustscript_differential::generator::generate;
use rustscript_differential::lang::catalog::{METHODS, solve};
use rustscript_differential::lang::ty::{INT_WIDTHS, SCALAR_TYPES, StdErr, Ty};
use rustscript_differential::model::Program;
use rustscript_differential::runner::{Classification, Runner};
use rustscript_differential::workspace_root;

#[test]
fn generation_is_deterministic() {
    for seed in [1, 7, 99, 4242] {
        assert_eq!(generate(seed), generate(seed));
        assert_eq!(generate(seed).render(), generate(seed).render());
    }
}

/// Every generated program must be valid Rust.
#[test]
fn generated_programs_compile() {
    let root = workspace_root();
    let runner = Runner::build(&root, 20_000).expect("build interpreter");
    let mut rejected = Vec::new();
    for seed in 0..60u64 {
        let source = generate(seed).render();
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

/// Every shape that can hide a real divergence. See `generation_covers_the_language`.
const EXPECTED_FEATURES: &[&str] = &[
    // types
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
    "lang-ty-tuple",
    "lang-ty-result",
    "lang-ty-stderr",
    "lang-ty-struct",
    "lang-ty-enum",
    "lang-ty-trace",
    // ownership
    "lang-trace-lit",
    "lang-move",
    "lang-move-field",
    "lang-mem-take",
    "lang-mem-replace",
    "lang-mem-swap",
    "lang-opt-take-expr",
    "lang-vec-pop-expr",
    "lang-vec-remove",
    "lang-vec-swap-remove",
    "lang-assign-field",
    "lang-scope",
    "vec_into_next",
    "vec_into_nth",
    "vec_into_last",
    // bindings and inference sites
    "lang-let",
    "lang-let-inferred",
    "lang-let-tuple",
    "lang-bare-int",
    "lang-bare-float",
    "lang-const",
    "lang-const-def",
    "lang-pipe-collect-fish",
    "lang-pipe-collect-bare",
    "lang-pipe-sum",
    "lang-pipe-sum-bare",
    "lang-pipe-product",
    "lang-pipe-param-inferred",
    "lang-pipe-fold",
    "lang-pipe-step-by",
    "lang-pipe-last",
    "lang-pipe-all",
    // functions and closures
    "lang-fn-def",
    "lang-fn-call",
    "lang-fn-writer",
    "lang-borrow-mut",
    "lang-fn-generic",
    "lang-fn-apply",
    "lang-apply-call",
    "lang-fn-factory",
    "lang-closure",
    "lang-closure-call",
    "lang-closure-move",
    "lang-closure-mut",
    "lang-closure-factory",
    "lang-early-return",
    "lang-try",
    // control flow
    "lang-if",
    "lang-if-stmt",
    "lang-for",
    "lang-while",
    "lang-loop",
    "lang-break",
    "lang-continue",
    "lang-for-accum",
    "lang-iter-mut",
    "lang-compound",
    "lang-match",
    "lang-pat-range",
    "lang-pat-guard",
    "lang-pat-slice",
    "lang-pat-slice-rest",
    "lang-pat-tuple",
    "lang-pat-enum",
    "lang-pat-option",
    "lang-pat-result",
    "lang-pat-struct",
    // user types
    "lang-struct-def",
    "lang-enum-def",
    "lang-struct-lit",
    "lang-struct-update",
    "lang-enum-lit",
    "lang-default",
    "lang-field",
    "lang-tuple-field",
    "lang-index",
    "lang-method",
    "lang-method-def",
    "lang-assoc-fn",
    "lang-display-impl",
    "lang-from-impl",
    "lang-from",
    "lang-into",
    "lang-trait-impl",
    "lang-trait-impl-builtin",
    "lang-trait-call",
    // operators and calls
    "lang-call",
    "lang-cast",
    "lang-op-add",
    "lang-op-shl",
    "lang-op-compare",
    "lang-mut-map-insert",
    "lang-mut-retain",
    "lang-mut-swap",
    "lang-mut-opt-take",
    "lang-mut-str-push-str",
    // formatting
    "lang-fmt-display",
    "lang-fmt-debug",
    "lang-fmt-hex",
    "lang-fmt-binary",
    "lang-fmt-exp",
    "lang-fmt-width",
    "lang-fmt-align",
    "lang-fmt-plus",
    "lang-fmt-zero",
    "lang-fmt-precision",
    "lang-fmt-alternate",
    "lang-print-indexed",
    "lang-print-twice",
    "lang-print-width-arg",
    "lang-print-named-width",
];

/// If a feature stops appearing, bugs living there become unfindable again.
#[test]
fn generation_covers_the_language() {
    let mut features = BTreeSet::new();
    for seed in 0..400u64 {
        features.extend(generate(seed).structural_features());
    }
    let expected = EXPECTED_FEATURES;
    let missing: Vec<&str> = expected
        .iter()
        .copied()
        .filter(|name| !features.contains(name))
        .collect();
    assert!(missing.is_empty(), "never generated: {missing:?}");
}

/// A method no wanted type can solve against is dead weight, the generator can never place a call to it.
#[test]
fn every_catalog_method_is_reachable() {
    let mut wanted: Vec<Ty> = SCALAR_TYPES.to_vec();
    for width in INT_WIDTHS {
        wanted.push(Ty::vec_of(Ty::Int(*width)));
        wanted.push(Ty::opt_of(Ty::Int(*width)));
        wanted.push(Ty::Tuple(vec![Ty::Int(*width), Ty::Bool]));
    }
    for scalar in SCALAR_TYPES {
        wanted.push(Ty::vec_of(scalar.clone()));
        wanted.push(Ty::opt_of(scalar.clone()));
        wanted.push(Ty::vec_of(Ty::vec_of(scalar.clone())));
        wanted.push(Ty::res_of(scalar.clone(), Ty::Str));
        wanted.push(Ty::res_of(scalar.clone(), Ty::StdErr(StdErr::ParseInt)));
        wanted.push(Ty::opt_of(Ty::Tuple(vec![scalar.clone(), scalar.clone()])));
        wanted.push(Ty::vec_of(Ty::Tuple(vec![scalar.clone(), scalar.clone()])));
        wanted.push(Ty::vec_of(Ty::Tuple(vec![Ty::USIZE, scalar.clone()])));
        wanted.push(Ty::Tuple(vec![
            Ty::vec_of(scalar.clone()),
            Ty::vec_of(scalar.clone()),
        ]));
        wanted.push(Ty::opt_of(Ty::Tuple(vec![
            scalar.clone(),
            Ty::vec_of(scalar.clone()),
        ])));
        wanted.push(Ty::res_of(Ty::USIZE, Ty::USIZE));
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

/// A duplicate key would silently make 1 row render as another.
#[test]
fn catalog_names_are_unique() {
    let mut seen = BTreeSet::new();
    for method in METHODS.iter() {
        assert!(
            seen.insert(method.name),
            "duplicate catalog name `{}`",
            method.name
        );
    }
}

/// A closure that can panic must not run on an unordered stretch, real Rust randomizes map and set order
/// so the panic message changes per run. A nightly campaign hit this with `into_values()` sorted
/// only after the map.
#[test]
fn fallible_closure_needs_defined_order() {
    use rustscript_differential::lang::expr::{BinOp, Expr};
    use rustscript_differential::lang::pipe::{
        Access, Bind, ParamAnn, Pipe, Site, Source, Stage, Term,
    };
    use rustscript_differential::lang::ty::IntWidth;

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
        ann: ParamAnn::Typed,
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

    // the nightly's failing shape
    assert!(!pipe_with(vec![fallible_map.clone(), Stage::Sorted]).is_deterministic());
    // sorting first is fine
    assert!(pipe_with(vec![Stage::Sorted, fallible_map]).is_deterministic());
}

/// A `Skip` past the end can drop every closure call before it. std collects a `Vec` into a `Vec` in
/// place and touches no item when the length is zero, so a panicking body never runs there while
/// the lazy chain runs it.
#[test]
fn fallible_closure_must_not_hide_behind_skip() {
    use rustscript_differential::lang::expr::{BinOp, Expr};
    use rustscript_differential::lang::pipe::{
        Access, Bind, ParamAnn, Pipe, Site, Source, Stage, Term,
    };
    use rustscript_differential::lang::ty::IntWidth;

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
        ann: ParamAnn::Typed,
    };
    let pipe_with = |stages: Vec<Stage>| Pipe {
        source: Source::Coll {
            expr: Expr::VecLit {
                elem: Ty::I64,
                items: vec![int(1)],
            },
            access: Access::VecInto,
        },
        stages,
        term: Term::Collect {
            target: Ty::vec_of(Ty::I64),
            site: Site::Turbofish,
        },
    };

    // the nightly's failing shape
    assert!(!pipe_with(vec![fallible_map.clone(), Stage::Skip(2)]).is_valid());
    // skipping first is fine
    assert!(pipe_with(vec![Stage::Skip(2), fallible_map.clone()]).is_valid());
    // a `Sorted` forces the body to run
    assert!(pipe_with(vec![fallible_map.clone(), Stage::Sorted, Stage::Skip(2)]).is_valid());
    // other length changing stages are fine
    assert!(pipe_with(vec![fallible_map, Stage::Take(2)]).is_valid());
}

/// Every shrink candidate must parse, so the reducer never trades a finding for a harness error.
#[test]
fn shrink_candidates_parse() {
    for seed in 0..8u64 {
        let program: Program = generate(seed);
        let candidates = program.shrink_candidates();
        let last_start = candidates.len().saturating_sub(24);
        let sample = candidates
            .iter()
            .take(24)
            .chain(candidates.iter().skip(last_start));
        for candidate in sample {
            let source = candidate.render();
            syn::parse_file(&source).unwrap_or_else(|error| {
                panic!("seed {seed} shrink did not parse: {error}\n{source}");
            });
        }
    }
}

/// The reducer must end even when every candidate reproduces. Cycling between 2 same size forms
/// runs for millions of steps.
#[test]
fn reduction_terminates_when_everything_reproduces() {
    use rustscript_differential::reduce::reduce_by;
    use rustscript_differential::runner::{ProcessOutput, RunResult};
    let output = ProcessOutput {
        status: Some(0),
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
    };
    let target = RunResult {
        classification: Classification::SemanticMismatch,
        compiler: output.clone(),
        native: output.clone(),
        interpreted: output,
    };
    for seed in 0..4u64 {
        let program: Program = generate(seed);
        let original_len = program.render().len();
        let (reduced, _) = reduce_by(
            |_| Ok(target.clone()),
            &program,
            &target,
            |progress| {
                assert!(
                    progress.candidates_checked < 200_000,
                    "seed {seed}: the reducer does not converge"
                );
            },
        )
        .expect("reduction runs");
        assert!(reduced.render().len() < original_len);
    }
}

/// A row naming a method only the interpreter invented would test the interpreter against itself.
#[test]
fn catalog_calls_are_std() {
    use rustscript_differential::surface::{TRAIT_METHODS, load, template_methods};
    let surface = load(&workspace_root()).expect("std_surface.txt, run `surface --refresh`");
    let std_names: BTreeSet<&String> = surface.values().flatten().collect();
    let mut unknown: Vec<String> = METHODS
        .iter()
        .flat_map(|method| template_methods(method.template))
        .filter(|name| !std_names.contains(name) && !TRAIT_METHODS.contains(&name.as_str()))
        .collect();
    unknown.sort();
    unknown.dedup();
    assert!(
        unknown.is_empty(),
        "catalog calls that are not std: {unknown:?}"
    );
}
