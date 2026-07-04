#!/usr/bin/env rust

// Random number generation. Only stable properties are printed so the output
// does not change from run to run.

use rand::RngExt;

fn main() {
    let mut rng = rand::rng();

    let n = rng.random_range(0..100);
    println!("range 0..100 respected: {}", n >= 0 && n < 100);

    let f = rng.random::<f64>();
    println!("unit float in range: {}", f >= 0.0 && f < 1.0);

    let mut buf = vec![0u8; 16];
    rng.fill(&mut buf);
    println!("filled 16 bytes: {}", buf.len() == 16);
}
