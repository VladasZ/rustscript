#!/usr/bin/env rust

fn main() {
    println!("{:>6} {:>8}", "C", "F");
    for c in [0, 20, 37, 100] {
        let f = f64::from(c) * 9.0 / 5.0 + 32.0;
        println!("{c:>6} {f:>8.1}");
    }
}
