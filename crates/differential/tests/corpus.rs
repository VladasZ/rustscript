//! Replays every case under `corpus/` through the compiler and the
//! interpreter and requires full agreement, panic payloads included. This is
//! the permanent guard for panic-vein findings, which the equivalence suite
//! cannot hold because it requires examples to exit cleanly.

use rustscript_differential::runner::{Classification, Runner};
use rustscript_differential::workspace_root;

#[test]
fn corpus_cases_still_agree() {
    let root = workspace_root();
    let corpus = root.join("crates/differential/corpus");
    let runner = Runner::build(&root, 10_000).expect("build interpreter");
    let mut checked = 0;
    for entry in std::fs::read_dir(&corpus).expect("read corpus directory") {
        let path = entry.expect("corpus entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read corpus case");
        let result = runner.run_source(&source).expect("run corpus case");
        assert_eq!(
            result.classification,
            Classification::Match,
            "corpus case {} diverged:\n-- native stdout --\n{}\n-- native stderr --\n{}\n-- interpreted stdout --\n{}\n-- interpreted stderr --\n{}",
            path.display(),
            result.native.stdout,
            result.native.stderr,
            result.interpreted.stdout,
            result.interpreted.stderr,
        );
        checked += 1;
    }
    assert!(checked > 0, "the corpus directory has no cases");
}
