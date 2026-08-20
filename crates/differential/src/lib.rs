pub mod artifact;
pub mod generator;
pub mod lang;
pub mod model;
pub mod mutator;
pub mod reduce;
pub mod runner;
pub mod surface;

use std::path::{Path, PathBuf};

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}
