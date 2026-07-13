use serde::Serialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::time::Instant;

#[derive(Serialize)]
struct Entry {
    token: String,
    count: i64,
}

#[derive(Serialize)]
struct Report {
    total: i64,
    unique: usize,
    top: Vec<Entry>,
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let config_path = &args[1];
    let input_path = &args[2];
    let output_path = &args[3];
    let t = Instant::now();
    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(config_path).unwrap()).unwrap();
    let text = fs::read_to_string(input_path).unwrap();
    let regex = regex::Regex::new(config["pattern"].as_str().unwrap()).unwrap();
    let limit = config["top"].as_i64().unwrap() as usize;
    let mut counts: HashMap<String, i64> = HashMap::new();
    let mut total: i64 = 0;
    for found in regex.find_iter(&text) {
        let token = found.as_str();
        let count = counts.get(token).copied().unwrap_or(0) + 1;
        counts.insert(token.to_string(), count);
        total += 1;
    }
    let unique = counts.len();
    let mut pairs: Vec<(String, i64)> = counts.into_iter().collect();
    pairs.sort_by(|a, b| {
        if a.1 == b.1 {
            a.0.cmp(&b.0)
        } else {
            b.1.cmp(&a.1)
        }
    });
    let mut top = Vec::new();
    for pair in pairs.into_iter().take(limit) {
        top.push(Entry {
            token: pair.0,
            count: pair.1,
        });
    }
    let report = Report { total, unique, top };
    fs::write(output_path, serde_json::to_string(&report).unwrap()).unwrap();
    let saved = fs::read_to_string(output_path).unwrap();
    let mut checksum: u64 = 0;
    for byte in saved.bytes() {
        checksum = (checksum + byte as u64) % 1_000_000_007;
    }
    let ns = t.elapsed().as_nanos();
    println!(
        "total={total} unique={unique} bytes={} checksum={checksum}",
        saved.len()
    );
    eprintln!("COMPUTE_NS {ns}");
}
