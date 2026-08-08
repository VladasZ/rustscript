use std::collections::HashMap;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: i64 = if args.len() > 1 {
        args[1].parse().unwrap()
    } else {
        150_000
    };
    let t = Instant::now();
    // minstd LCG, exact in f64 so every language generates the same sequence.
    let mut x: i64 = 12345;
    let mut counts: HashMap<i64, i64> = HashMap::new();
    for _ in 0..n {
        x = x * 48271 % 2_147_483_647;
        let key = x % 65536;
        let next = counts.get(&key).copied().unwrap_or(0) + 1;
        counts.insert(key, next);
    }
    let mut total: i64 = 0;
    let mut hits: i64 = 0;
    for key in 0..65536 {
        if let Some(bucket) = counts.get(&key) {
            total += *bucket;
            hits += 1;
        }
    }
    let ns = t.elapsed().as_nanos();
    println!("keys={} hits={hits} total={total}", counts.len());
    eprintln!("COMPUTE_NS {ns}");
}
