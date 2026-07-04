//! Shared types for the benchmark report, written by `bench` and read by
//! `chart`. All times are in seconds, memory in bytes.

use serde::{Deserialize, Serialize};

/// The four contestants, in a fixed order used everywhere for stable colors.
pub const LANGS: [&str; 4] = ["native", "rustscript", "node", "python"];

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

/// Peak resident set size of one run, the max over samples.
#[derive(Serialize, Deserialize, Clone)]
pub struct MemStat {
    pub lang: String,
    pub rss_bytes: u64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct CaseResult {
    pub name: String,
    /// "startup" for hello and big_script, "compute" for the rest.
    pub kind: String,
    /// "base" or "big". The big tier runs the same case at 10x size, where
    /// startup and JIT warmup stop dominating.
    #[serde(default = "default_tier")]
    pub tier: String,
    pub wall: Vec<WallStat>,
    pub compute: Vec<CompStat>,
    #[serde(default)]
    pub mem: Vec<MemStat>,
}

fn default_tier() -> String {
    "base".to_string()
}

impl CaseResult {
    pub fn wall_of(&self, lang: &str) -> Option<&WallStat> {
        self.wall.iter().find(|w| w.lang == lang)
    }
    pub fn compute_of(&self, lang: &str) -> Option<&CompStat> {
        self.compute.iter().find(|c| c.lang == lang)
    }
    pub fn mem_of(&self, lang: &str) -> Option<&MemStat> {
        self.mem.iter().find(|m| m.lang == lang)
    }
}

/// The one-time `cargo check` gate cost, measured on a trivial script so the
/// number reflects the gate itself and not any real work.
#[derive(Serialize, Deserialize, Clone)]
pub struct Gate {
    pub cold_mean: f64,
    pub warm_mean: f64,
}

/// What produced the numbers, so old and new runs stay comparable.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Meta {
    pub date_unix: u64,
    pub rustc: String,
    pub node: String,
    pub python: String,
    pub os: String,
    pub arch: String,
    pub cpu: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Report {
    #[serde(default)]
    pub meta: Meta,
    pub cases: Vec<CaseResult>,
    pub gate: Gate,
}
