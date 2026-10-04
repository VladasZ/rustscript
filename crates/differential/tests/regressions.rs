//! Replays every case under `regressions/` compiled and interpreted and requires full agreement, panic
//! payloads included. The equivalence suite can't hold panicking cases.

use std::fs::{read_dir, read_to_string};
use std::path::PathBuf;

use rustscript_differential::parallel::map;
use rustscript_differential::runner::{Classification, Runner};
use rustscript_differential::workspace_root;

#[test]
fn regression_cases_still_agree() {
    let root = workspace_root();
    let regressions = root.join("crates/differential/regressions");
    let runner = Runner::build(&root, 10_000).expect("build interpreter");
    let mut cases: Vec<PathBuf> = read_dir(&regressions)
        .expect("read regressions directory")
        .map(|entry| entry.expect("regressions entry").path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("rs"))
        .collect();
    cases.sort();
    assert!(!cases.is_empty(), "the regressions directory has no cases");
    let diverged: Vec<String> = map(&cases, |path| {
        let source = read_to_string(path).expect("read regression case");
        let result = runner.run_source(&source).expect("run regression case");
        (result.classification != Classification::Match).then(|| {
            format!(
                "regression case {} diverged:\n-- native stdout --\n{}\n-- native stderr --\n{}\n-- interpreted stdout --\n{}\n-- interpreted stderr --\n{}",
                path.display(),
                result.native.stdout,
                result.native.stderr,
                result.interpreted.stdout,
                result.interpreted.stderr,
            )
        })
    })
    .into_iter()
    .flatten()
    .collect();
    assert!(diverged.is_empty(), "{}", diverged.join("\n\n"));
}
