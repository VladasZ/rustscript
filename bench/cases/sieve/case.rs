use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: usize = if args.len() > 1 { args[1].parse().unwrap() } else { 250_000 };
    let t = Instant::now();
    let mut is_prime = vec![true; n + 1];
    is_prime[0] = false;
    is_prime[1] = false;
    let mut i = 2;
    while i * i <= n {
        if is_prime[i] {
            let mut j = i * i;
            while j <= n {
                is_prime[j] = false;
                j += i;
            }
        }
        i += 1;
    }
    let mut count: u64 = 0;
    for k in 2..=n {
        if is_prime[k] {
            count += 1;
        }
    }
    let ns = t.elapsed().as_nanos();
    println!("primes up to {n}: {count}");
    eprintln!("COMPUTE_NS {ns}");
}
