pub mod artifact;
pub mod closure_case;
pub mod generator;
pub mod model;
pub mod mutator;
pub mod reduce;
pub mod rich;
pub mod runner;
pub mod semantic;
pub mod semantic_gen;
pub mod structural;
pub mod structural_gen;
pub mod typed;
pub mod typed_gen;
mod typed_shrink;

use std::path::{Path, PathBuf};

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}
