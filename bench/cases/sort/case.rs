use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: i64 = if args.len() > 1 {
        args[1].parse().unwrap()
    } else {
        50_000
    };
    let t = Instant::now();
    // minstd LCG, exact in f64 so every language generates the same sequence.
    let mut seed: i64 = 12345;
    let mut values: Vec<i64> = Vec::new();
    for _ in 0..n {
        seed = seed * 48271 % 2_147_483_647;
        values.push(seed % 1_000_000);
    }
    // Sort through a comparison callback, bucket first, value second.
    values.sort_by(|a, b| {
        if a % 1000 == b % 1000 {
            a.cmp(b)
        } else {
            (a % 1000).cmp(&(b % 1000))
        }
    });
    let len = values.len();
    let mut probe: i64 = 0;
    let mut i = 0;
    while i < len {
        probe += values[i];
        i += len / 10;
    }
    let ns = t.elapsed().as_nanos();
    println!(
        "first={} mid={} last={} probe={probe}",
        values[0],
        values[len / 2],
        values[len - 1]
    );
    eprintln!("COMPUTE_NS {ns}");
}
