//! The gate on the generator's reach. A std method the interpreter implements on a type the
//! generator writes must show up in generated programs, or stand in `surface_skips.txt` with a
//! reason. So a new bridge method can't land without a differential case that calls it.

use std::collections::BTreeSet;

use rustscript_differential::runner::Runner;
use rustscript_differential::surface::{generated_names, load, load_skips, ungenerated};
use rustscript_differential::workspace_root;

/// Enough seeds for every catalog row and pipe stage to appear.
const SAMPLE: u64 = 3000;

#[test]
fn every_implemented_std_method_is_generated_or_skipped() {
    let root = workspace_root();
    let surface = load(&root).expect("std surface");
    let runner = Runner::build(&root, 20_000).expect("build interpreter");
    let listing = runner.supported_listing().expect("supported listing");
    let generated = generated_names(0..SAMPLE);
    let missing = ungenerated(&surface, &listing, &generated);
    let skips = load_skips(&root).expect("skips");
    let skipped: BTreeSet<(String, String)> = skips.keys().cloned().collect();

    let unexplained: Vec<String> = missing
        .difference(&skipped)
        .map(|(group, name)| format!("{group} {name}"))
        .collect();
    assert!(
        unexplained.is_empty(),
        "{} std method(s) the interpreter implements and no generated program calls. Add a catalog \
         row or a generator form for each, or a line with a reason in surface_skips.txt:\n{}",
        unexplained.len(),
        unexplained.join("\n")
    );

    let stale: Vec<String> = skipped
        .difference(&missing)
        .map(|(group, name)| format!("{group} {name}"))
        .collect();
    assert!(
        stale.is_empty(),
        "surface_skips.txt lists methods that are generated now or are gone, delete the lines:\n{}",
        stale.join("\n")
    );
}
