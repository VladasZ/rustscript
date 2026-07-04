use std::time::Instant;

fn fib(n: u64) -> u64 {
    if n < 2 {
        n
    } else {
        fib(n - 1) + fib(n - 2)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: u64 = if args.len() > 1 { args[1].parse().unwrap() } else { 27 };
    let t = Instant::now();
    let r = fib(n);
    let ns = t.elapsed().as_nanos();
    println!("fib({n}) = {r}");
    eprintln!("COMPUTE_NS {ns}");
}
