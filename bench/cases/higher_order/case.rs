use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: i64 = if args.len() > 1 {
        args[1].parse().unwrap()
    } else {
        100_000
    };
    let t = Instant::now();
    // minstd LCG, exact in f64 so every language generates the same sequence.
    let mut x: i64 = 12345;
    let mut v: Vec<i64> = Vec::new();
    for _ in 0..n {
        x = x * 48271 % 2147483647;
        v.push(x % 1000);
    }
    let sum: i64 = v.iter().map(|a| a * 3 + 1).filter(|a| a % 2 == 0).sum();
    let count = v.iter().filter(|a| **a > 500).count();
    let any_big = v.iter().any(|a| *a > 995);
    let ns = t.elapsed().as_nanos();
    println!("sum={sum} count={count} any={any_big}");
    eprintln!("COMPUTE_NS {ns}");
}
