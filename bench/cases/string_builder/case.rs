use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: i64 = if args.len() > 1 { args[1].parse().unwrap() } else { 200_000 };
    let t = Instant::now();
    let mut s = String::new();
    let mut i: i64 = 0;
    while i < n {
        s.push_str("item");
        s.push_str(&i.to_string());
        s.push_str(" ");
        i += 1;
    }
    let hits = s.split("item12").count() - 1;
    let replaced = s.replace("item9", "ITEM");
    let ns = t.elapsed().as_nanos();
    println!("len={} hits={hits} rlen={}", s.len(), replaced.len());
    eprintln!("COMPUTE_NS {ns}");
}
