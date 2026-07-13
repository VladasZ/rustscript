use serde::{Deserialize, Serialize};

pub mod http_server;
pub mod provenance;
pub mod sample;

pub const LANGS: [&str; 4] = ["native", "rustscript", "node", "python"];

#[derive(Serialize, Deserialize, Clone)]
pub struct TimeStat {
    pub lang: String,
    pub median: f64,
    pub samples: Vec<f64>,
}

impl TimeStat {
    pub fn from_samples(lang: impl Into<String>, samples: Vec<f64>) -> Self {
        let median = median_f64(&samples);
        Self {
            lang: lang.into(),
            median,
            samples,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct MemStat {
    pub lang: String,
    pub median_bytes: u64,
    pub samples: Vec<u64>,
}

impl MemStat {
    pub fn from_samples(lang: impl Into<String>, samples: Vec<u64>) -> Self {
        let median_bytes = median_u64(&samples);
        Self {
            lang: lang.into(),
            median_bytes,
            samples,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct CaseResult {
    pub name: String,
    pub kind: String,
    pub tier: String,
    pub parameters: Vec<String>,
    pub wall: Vec<TimeStat>,
    pub compute: Vec<TimeStat>,
    pub memory: Vec<MemStat>,
}

impl CaseResult {
    pub fn wall_of(&self, lang: &str) -> Option<&TimeStat> {
        self.wall.iter().find(|sample| sample.lang == lang)
    }

    pub fn compute_of(&self, lang: &str) -> Option<&TimeStat> {
        self.compute.iter().find(|sample| sample.lang == lang)
    }

    pub fn memory_of(&self, lang: &str) -> Option<&MemStat> {
        self.memory.iter().find(|sample| sample.lang == lang)
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Gate {
    pub warm_median: f64,
    pub warm_samples: Vec<f64>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct NamedHash {
    pub name: String,
    pub sha256: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Settings {
    pub warmups: u32,
    pub wall_samples: u32,
    pub compute_samples: u32,
    pub quick: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Meta {
    pub date_unix: u64,
    pub git_commit: String,
    pub git_dirty: bool,
    pub rustscript_binary: String,
    pub rustscript_sha256: String,
    pub benchmark_sha256: String,
    pub fixtures: Vec<NamedHash>,
    pub rustc: String,
    pub cargo: String,
    pub node: String,
    pub python: String,
    pub os: String,
    pub arch: String,
    pub cpu: String,
    pub settings: Settings,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Report {
    pub schema_version: u32,
    pub meta: Meta,
    pub cases: Vec<CaseResult>,
    pub gate: Gate,
}

pub fn median_f64(samples: &[f64]) -> f64 {
    assert!(!samples.is_empty(), "median needs samples");
    let mut sorted = samples.to_vec();
    sorted.sort_by(|left, right| left.total_cmp(right));
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    }
}

pub fn median_u64(samples: &[u64]) -> u64 {
    assert!(!samples.is_empty(), "median needs samples");
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        sorted[middle - 1] / 2
            + sorted[middle] / 2
            + (sorted[middle - 1] % 2 + sorted[middle] % 2) / 2
    } else {
        sorted[middle]
    }
}
