#!/usr/bin/env rust

// only stable properties are printed

use rand::{RngExt, rng};

fn main() {
    let mut rng = rng();

    let n = rng.random_range(0..100);
    println!("range 0..100 respected: {}", (0..100).contains(&n));

    let f = rng.random::<f64>();
    println!("unit float in range: {}", f.is_sign_positive() && f < 1.0);

    let mut buf = vec![0u8; 16];
    rng.fill(&mut buf);
    println!("filled 16 bytes: {}", buf.len() == 16);
}
