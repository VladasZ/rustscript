use std::collections::BTreeSet;
use std::process::Command;

use rustscript_differential::generator::generate;
use rustscript_differential::lang::expr::Expr as MatchExpr;

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

/// A splice must land now and then, or every mutated seed is only its parent with the blocks reversed.
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

/// Every 4th seed is a mutation of its predecessor, so only base seeds count.
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

#[test]
fn splice_never_targets_a_small_count_argument() {
    use rustscript_differential::lang::expr::Expr;
    use rustscript_differential::lang::ty::Ty;
    use rustscript_differential::lang::width::IntWidth;
    let count = Expr::IntLit {
        width: IntWidth::USize,
        value: 2,
        opaque: true,
    };
    let receiver = Expr::VecLit {
        elem: Ty::Int(IntWidth::I64),
        items: vec![Expr::IntLit {
            width: IntWidth::I64,
            value: 1,
            opaque: false,
        }],
    };
    let call = Expr::Call {
        method: "vec_repeat".to_string(),
        recv: Box::new(receiver),
        args: vec![count],
        fish: None,
        ty: Ty::Vec(Box::new(Ty::Int(IntWidth::I64))),
    };
    // the call, the receiver vec, its item, then the count
    assert_eq!(call.pinned_nodes(), vec![false, false, false, true]);
}

/// Seed 2852428022 bound a bare int scrutinee and called `saturating_pow` on the binding, which
/// `rustc` rejects as an ambiguous numeric type. A match that binds must state its literal widths.
#[test]
fn bound_match_scrutinees_carry_their_widths() {
    for seed in 0..1_000 {
        let program = generate(seed);
        for block in &program.blocks {
            let stmt_exprs = block.statements.iter().flat_map(|s| s.exprs());
            let fn_exprs = block.fns.iter().flat_map(|f| f.exprs());
            for expr in stmt_exprs.chain(fn_exprs) {
                for node in expr.nodes() {
                    let MatchExpr::Match {
                        scrutinee, arms, ..
                    } = node
                    else {
                        continue;
                    };
                    let mut binds = Vec::new();
                    for arm in arms {
                        arm.pat.bindings(&mut binds);
                    }
                    if binds.is_empty() {
                        continue;
                    }
                    let bare = scrutinee.nodes().into_iter().any(|n| {
                        matches!(n, MatchExpr::BareInt { .. } | MatchExpr::BareFloat { .. })
                    });
                    assert!(
                        !bare,
                        "seed {seed} binds a bare match scrutinee:\n{}",
                        node.render()
                    );
                }
            }
        }
    }
}
