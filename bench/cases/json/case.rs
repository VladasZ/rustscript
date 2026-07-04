use std::fs;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args[1].clone();
    let text = fs::read_to_string(&path).unwrap();
    let t = Instant::now();
    // Parse into dynamic values, the same work node and python do. A typed
    // serde struct would skip the value tree they both must build.
    let items: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
    let mut sum: i64 = 0;
    let mut ids: i64 = 0;
    for it in &items {
        sum += it["value"].as_i64().unwrap();
        ids += it["id"].as_i64().unwrap();
    }
    let count = items.len();
    let ns = t.elapsed().as_nanos();
    println!("count={count} sum={sum} ids={ids}");
    eprintln!("COMPUTE_NS {ns}");
}
