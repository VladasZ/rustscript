use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: i64 = if args.len() > 1 {
        args[1].parse().unwrap()
    } else {
        50_000
    };
    let t = Instant::now();
    let mut x: i64 = 12345;
    let mut values: Vec<i64> = Vec::new();
    for _ in 0..n {
        x = x * 48271 % 2147483647;
        values.push(x % 1_000_000);
    }
    values.sort_by_cached_key(|value| (value % 1000, *value));
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
