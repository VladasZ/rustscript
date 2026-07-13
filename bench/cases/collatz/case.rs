use std::time::Instant;

fn steps(start: u64) -> u64 {
    let mut n = start;
    let mut c: u64 = 0;
    while n != 1 {
        if n.is_multiple_of(2) {
            n /= 2;
        } else {
            n = 3 * n + 1;
        }
        c += 1;
    }
    c
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let limit: u64 = if args.len() > 1 {
        args[1].parse().unwrap()
    } else {
        10_000
    };
    let t = Instant::now();
    let mut total: u64 = 0;
    for i in 1..=limit {
        total += steps(i);
    }
    let ns = t.elapsed().as_nanos();
    println!("total steps for 1..{limit}: {total}");
    eprintln!("COMPUTE_NS {ns}");
}
