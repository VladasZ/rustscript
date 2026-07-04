use std::collections::HashMap;
use std::fs;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args[1].clone();
    let text = fs::read_to_string(&path).unwrap();
    let t = Instant::now();
    let mut counts: HashMap<String, i64> = HashMap::new();
    for w in text.split_whitespace() {
        let n = counts.get(w).copied().unwrap_or(0) + 1;
        counts.insert(w.to_string(), n);
    }
    let mut pairs: Vec<(String, i64)> = counts.into_iter().collect();
    pairs.sort_by_key(|p| (-p.1, p.0.clone()));
    let ns = t.elapsed().as_nanos();
    for i in 0..15 {
        println!("{} {}", pairs[i].0, pairs[i].1);
    }
    eprintln!("COMPUTE_NS {ns}");
}
