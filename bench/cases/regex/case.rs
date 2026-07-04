use std::fs;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args[1].clone();
    let text = fs::read_to_string(&path).unwrap();
    let t = Instant::now();
    let re_low = regex::Regex::new(r"w0\d\d").unwrap();
    let mut matches: i64 = 0;
    let mut spans: i64 = 0;
    for m in re_low.find_iter(&text) {
        matches += 1;
        spans += m.start() as i64 % 1000;
    }
    let re_cap = regex::Regex::new(r"w(\d)(\d)9\d").unwrap();
    let mut digits: i64 = 0;
    for caps in re_cap.captures_iter(&text) {
        let a: i64 = caps.get(1).unwrap().as_str().parse().unwrap();
        let b: i64 = caps.get(2).unwrap().as_str().parse().unwrap();
        digits += a * 10 + b;
    }
    let re_sub = regex::Regex::new(r"w00\d").unwrap();
    let replaced = re_sub.replace_all(&text, "X");
    let ns = t.elapsed().as_nanos();
    println!("matches={matches} spans={spans} digits={digits} rlen={}", replaced.len());
    eprintln!("COMPUTE_NS {ns}");
}
