use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::model::Program;
use crate::runner::RunResult;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub schema: u32,
    pub saved_at_ms: u128,
    pub seed: u64,
    pub program: Program,
    pub source: String,
    pub result: RunResult,
}

impl Artifact {
    pub fn new(seed: u64, program: Program, source: String, result: RunResult) -> Self {
        Self {
            schema: 1,
            saved_at_ms: now_ms(),
            seed,
            program,
            source,
            result,
        }
    }

    pub fn save(&self, workspace: &Path) -> Result<PathBuf> {
        let parent = workspace
            .join("target/rustscript-differential/failures")
            .join(format!("seed-{}-{}", self.seed, self.saved_at_ms));
        self.save_under(&parent, "artifact")
    }

    pub fn save_under(&self, parent: &Path, name: &str) -> Result<PathBuf> {
        let directory = parent.join(name);
        fs::create_dir_all(&directory)?;
        fs::write(directory.join("case.rs"), &self.source)?;
        let path = directory.join("artifact.json");
        fs::write(&path, serde_json::to_vec_pretty(self)?)?;
        Ok(path)
    }

    pub fn load(path: &Path) -> Result<Self> {
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time is before Unix epoch")
        .as_millis()
}
