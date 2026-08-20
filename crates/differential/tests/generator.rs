use std::collections::BTreeSet;
use std::process::Command;

use rustscript_differential::generator::generate;

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

/// A splice must actually land now and then, otherwise every mutated seed is
/// only its parent with the blocks reversed.
#[test]
fn mutations_splice_subtrees() {
    use rustscript_differential::model::MutationOperation;
    let spliced = (4..200)
        .step_by(4)
        .map(generate)
        .filter(|program| {
            program
                .mutation
                .as_ref()
                .is_some_and(|origin| origin.operations.contains(&MutationOperation::Splice))
        })
        .count();
    assert!(spliced >= 20, "only {spliced} of 49 mutated seeds spliced");
}

/// Every fourth seed is a mutation of its predecessor and shares its shape,
/// so the count runs over the base seeds only.
#[test]
fn generation_varies_program_topology() {
    let base: Vec<u64> = (0..250).filter(|seed| seed % 4 != 0).collect();
    let signatures = base
        .iter()
        .map(|seed| generate(*seed).structural_signature())
        .collect::<BTreeSet<_>>();
    assert!(
        signatures.len() + 5 >= base.len(),
        "only {} distinct structural shapes from {} base seeds",
        signatures.len(),
        base.len()
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
