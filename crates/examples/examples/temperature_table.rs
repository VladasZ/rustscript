#!/usr/bin/env rust

fn main() {
    println!("{:>6} {:>8}", "C", "F");
    for c in [0, 20, 37, 100] {
        let f = c as f64 * 9.0 / 5.0 + 32.0;
        println!("{:>6} {:>8.1}", c, f);
    }
}
