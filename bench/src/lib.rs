//! Shared types for the benchmark report, written by `bench` and read by
//! `chart`. All times are in seconds.

use serde::{Deserialize, Serialize};

/// The four contestants, in a fixed order used everywhere for stable colors.
pub const LANGS: [&str; 4] = ["native", "rustscript", "bun", "python"];

#[derive(Serialize, Deserialize, Clone)]
pub struct WallStat {
    pub lang: String,
    pub mean: f64,
    pub stddev: f64,
    pub min: f64,
    pub median: f64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct CompStat {
    pub lang: String,
    pub min: f64,
    pub median: f64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct CaseResult {
    pub name: String,
    /// "startup" for hello, "compute" for the rest.
    pub kind: String,
    pub wall: Vec<WallStat>,
    pub compute: Vec<CompStat>,
}

impl CaseResult {
    pub fn wall_of(&self, lang: &str) -> Option<&WallStat> {
        self.wall.iter().find(|w| w.lang == lang)
    }
    pub fn compute_of(&self, lang: &str) -> Option<&CompStat> {
        self.compute.iter().find(|c| c.lang == lang)
    }
}

/// The one-time `cargo check` gate cost, measured on a trivial script so the
/// number reflects the gate itself and not any real work.
#[derive(Serialize, Deserialize, Clone)]
pub struct Gate {
    pub cold_mean: f64,
    pub warm_mean: f64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Report {
    pub cases: Vec<CaseResult>,
    pub gate: Gate,
}
