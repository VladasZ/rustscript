use serde::Deserialize;
use std::fs;
use std::time::Instant;

#[derive(Deserialize)]
struct Item {
    id: i64,
    value: i64,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args[1].clone();
    let text = fs::read_to_string(&path).unwrap();
    let t = Instant::now();
    let items: Vec<Item> = serde_json::from_str(&text).unwrap();
    let mut sum: i64 = 0;
    let mut ids: i64 = 0;
    for it in &items {
        sum += it.value;
        ids += it.id;
    }
    let count = items.len();
    let ns = t.elapsed().as_nanos();
    println!("count={count} sum={sum} ids={ids}");
    eprintln!("COMPUTE_NS {ns}");
}
