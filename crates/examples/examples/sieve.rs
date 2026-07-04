#!/usr/bin/env rustscript

fn main() {
    let n: usize = 50;
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
    let primes: Vec<usize> = (2..=n).filter(|k| is_prime[*k]).collect();
    println!("primes up to {n}: {:?}", primes);
}
