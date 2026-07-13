use serde::Serialize;
use std::time::Instant;

#[derive(Serialize)]
struct Item {
    id: i64,
    value: i64,
    name: String,
}

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
    let mut items: Vec<Item> = Vec::new();
    for i in 0..n {
        x = x * 48271 % 2147483647;
        items.push(Item {
            id: i,
            value: x % 1000,
            name: format!("n{}", x % 10000),
        });
    }
    let out = serde_json::to_string(&items).unwrap();
    let ns = t.elapsed().as_nanos();
    println!("len={}", out.len());
    eprintln!("COMPUTE_NS {ns}");
}
