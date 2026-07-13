use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: i64 = if args.len() > 1 {
        args[1].parse().unwrap()
    } else {
        20_000
    };
    let t = Instant::now();
    let mut sum: i64 = 0;
    for i in 0..n {
        sum += i;
        println!("{i} {sum}");
    }
    let ns = t.elapsed().as_nanos();
    eprintln!("COMPUTE_NS {ns}");
}
