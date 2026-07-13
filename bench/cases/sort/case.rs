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
    let mut x: i64 = 12345;
    let mut v: Vec<i64> = Vec::new();
    for _ in 0..n {
        x = x * 48271 % 2147483647;
        v.push(x % 1_000_000);
    }
    // Sort through a per element callback, bucket first, value second.
    v.sort_by(|a, b| {
        if a % 1000 == b % 1000 {
            a.cmp(b)
        } else {
            (a % 1000).cmp(&(b % 1000))
        }
    });
    let len = v.len();
    let mut probe: i64 = 0;
    let mut i = 0;
    while i < len {
        probe += v[i];
        i += len / 10;
    }
    let ns = t.elapsed().as_nanos();
    println!(
        "first={} mid={} last={} probe={probe}",
        v[0],
        v[len / 2],
        v[len - 1]
    );
    eprintln!("COMPUTE_NS {ns}");
}
