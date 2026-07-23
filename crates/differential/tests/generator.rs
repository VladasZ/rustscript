use rustscript_differential::generator::generate;
use std::collections::BTreeSet;
use std::process::Command;

#[test]
fn generation_is_deterministic() {
    assert_eq!(generate(42), generate(42));
    assert_eq!(generate(42).render(), generate(42).render());
}

#[test]
fn generation_covers_typed_structural_surfaces() {
    let programs = (0..250).map(generate).collect::<Vec<_>>();
    let features = programs
        .iter()
        .flat_map(|program| program.structural_features())
        .collect::<BTreeSet<_>>();
    for expected in [
        "dataflow",
        "function",
        "mutable-closure",
        "enum",
        "match-enum",
        "match-guard",
        "for-loop",
        "type-i64",
        "type-bool",
        "type-string",
        "type-vec",
        "type-option",
        "vec-map",
        "vec-filter",
        "option-map",
        "option-filter",
        "match-option",
        "closure-call",
        "closure-owned-factory",
        "borrow-mut",
        "borrow-shared",
        "slice",
        "iter-mut",
        "struct",
        "associated-function",
        "method",
        "move",
        "result",
        "question-mark",
        "early-return",
        "iterator-enumerate",
        "iterator-filter-map",
        "iterator-take",
        "loop",
        "break",
        "continue",
    ] {
        assert!(
            features.contains(expected),
            "generated programs did not cover {expected:?}; got {features:?}"
        );
    }
}

#[test]
fn generation_includes_replayable_structured_mutations() {
    for seed in (4..100).step_by(4) {
        let first = generate(seed);
        let second = generate(seed);
        let origin = first
            .mutation
            .as_ref()
            .unwrap_or_else(|| panic!("seed {seed} was not mutated"));
        assert_eq!(origin.parent_seed, seed - 1);
        assert_eq!(origin.donor_seed, seed);
        assert!((2..=4).contains(&origin.operations.len()));
        assert_eq!(first, second);
    }
}

#[test]
fn generation_varies_closure_structure() {
    let sources: Vec<String> = (0..250).map(|seed| generate(seed).render()).collect();
    for expected in [
        "closure-nested",
        "closure-mutable",
        "closure-move",
        "closure-captured",
        "closure-tuple",
        "closure-generic",
        "move |right: i64|",
        "F: FnMut(i64) -> i64",
    ] {
        assert!(
            sources.iter().any(|source| source.contains(expected)),
            "generated sources did not contain {expected:?}"
        );
    }
}

#[test]
fn generation_varies_program_topology() {
    let signatures = (0..250)
        .map(|seed| generate(seed).structural_signature())
        .collect::<BTreeSet<_>>();
    assert!(
        signatures.len() >= 240,
        "only {} distinct structural shapes from 250 seeds",
        signatures.len()
    );
}

#[test]
fn generated_sources_parse_as_rust() {
    for seed in 0..1_000 {
        let source = generate(seed).render();
        syn::parse_file(&source).unwrap_or_else(|error| {
            panic!("seed {seed} did not parse: {error}\n{source}");
        });
    }
}

#[test]
fn generated_sources_compile_with_rustc() {
    let directory = tempfile::tempdir().unwrap();
    // The higher range covers seed 543626, where raw arithmetic once folded to
    // a constant divide by zero and the compiler rejected the program. The
    // `diff_opaque` shield keeps every raw operand out of const evaluation.
    for seed in (0..100).chain(543_600..543_660) {
        let source = generate(seed).render();
        let source_path = directory.path().join(format!("case_{seed}.rs"));
        let output_path = directory.path().join(format!("case_{seed}.rmeta"));
        std::fs::write(&source_path, &source).unwrap();
        let output = Command::new("rustc")
            .args(["--edition", "2024", "--emit", "metadata", "-o"])
            .arg(&output_path)
            .arg(&source_path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "seed {seed} did not compile:\n{}\n{source}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn shrink_candidates_still_parse() {
    for seed in 0..10 {
        let candidates = generate(seed).shrink_candidates();
        let last_start = candidates.len().saturating_sub(32);
        let sample = candidates
            .iter()
            .take(32)
            .chain(candidates.iter().skip(last_start));
        for candidate in sample {
            let source = candidate.render();
            syn::parse_file(&source).unwrap_or_else(|error| {
                panic!("seed {seed} shrink did not parse: {error}\n{source}");
            });
        }
    }
}
